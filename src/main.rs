#![feature(write_all_vectored)]
#![feature(lazy_cell)]

// TODO: add comments
// TODO: handle disconnections

mod config;
mod log;

use clap::Parser;
use config::{configure_client, generate_self_signed_cert};
use core::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use log::log;
use quinn::{Connection, Endpoint, ServerConfig};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use std::{
    collections::HashSet,
    io::{self, Write},
    sync::Arc,
};
use tokio::{
    sync::{
        broadcast::{self, Receiver, Sender},
        Mutex,
    },
    time::Instant,
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    period: Option<usize>,
    #[arg(long)]
    port: u16,
    #[arg(long)]
    connect: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), args.port);

    let (cert, key) = generate_self_signed_cert().unwrap();
    let mut endpoint = Endpoint::server(
        ServerConfig::with_single_cert(vec![cert], key).unwrap(),
        addr,
    )?;
    endpoint.set_default_client_config(configure_client());

    let (tx, _rx) = broadcast::channel::<String>(16);

    let peers = if let Some(connect) = args.connect {
        newcomer_connect(&endpoint, connect, &tx).await?
    } else {
        HashSet::new()
    };
    let peers = Arc::new(Mutex::new(peers));

    if let Some(period) = args.period {
        tokio::spawn(message_producing_loop(
            Duration::from_secs(period as _),
            peers.clone(),
            tx.clone(),
        ));
    }

    log(&[b"My address is \"", addr.to_string().as_bytes(), b"\""])?;

    while let Some(connection) = endpoint.accept().await {
        let connection = connection.await?;

        let (mut send, mut recv) = connection.accept_bi().await?;

        let mut is_newcomer = [0];
        recv.read_exact(&mut is_newcomer).await.unwrap();

        if is_newcomer == [0] {
            let set = peers.lock().await;
            send.write_all(&bincode::serialize(&*set).unwrap()).await?;
            drop(set);
            log(&[
                b"accepted a connection from ",
                connection.remote_address().to_string().as_bytes(),
            ])?;
        } else {
            // TODO: only send new peers
            let set = peers.lock().await;
            send.write_all(&bincode::serialize(&*set).unwrap()).await?;
            drop(set);
            log(&[
                b"accepted a newcomer connection from ",
                connection.remote_address().to_string().as_bytes(),
            ])?;
        }
        send.finish().await?;

        peers.lock().await.insert(connection.remote_address());
        tokio::spawn(handle_connection(connection, tx.subscribe()));
    }

    Ok(())
}

async fn newcomer_connect(
    endpoint: &Endpoint,
    connect: SocketAddr,
    tx: &Sender<String>,
) -> io::Result<HashSet<SocketAddr>> {
    // TODO: DNS lookup
    let connection = endpoint.connect(connect, "localhost").unwrap().await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&bincode::serialize(&true).unwrap()).await?;
    send.finish().await?;
    let line = recv.read_to_end(1024).await.unwrap();
    let mut peers: HashSet<SocketAddr> = bincode::deserialize(&line).unwrap();
    let mut new_peers = Vec::new();
    for addr in &peers {
        let connection = endpoint.connect(*addr, "localhost").unwrap().await?;

        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(&bincode::serialize(&false).unwrap()).await?;
        send.finish().await?;
        let line = recv.read_to_end(1024).await.unwrap();
        let new_peers_chunk: Vec<SocketAddr> = bincode::deserialize(&line).unwrap();
        new_peers.extend(new_peers_chunk);

        tokio::spawn(handle_connection(connection, tx.subscribe()));
    }
    peers.extend(new_peers);
    peers.insert(connect);
    tokio::spawn(handle_connection(connection, tx.subscribe()));
    Ok(peers)
}

async fn message_producing_loop(
    duration: Duration,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    tx: Sender<String>,
) -> io::Result<()> {
    fn generate_random_message(rng: &mut impl Rng) -> String {
        let mut message = [0; 32];
        rng.fill_bytes(&mut message);
        bs58::encode(message).into_string()
    }

    let mut rng = Pcg64Mcg::from_entropy();

    loop {
        let end = Instant::now() + duration;
        let mut to = Vec::new();
        for (i, addr) in peers.lock().await.iter().enumerate() {
            if i != 0 {
                to.extend_from_slice(b", ");
            }
            to.push(b'"');
            write!(&mut to, "{addr}")?;
            to.push(b'"');
        }

        if !to.is_empty() {
            let msg = generate_random_message(&mut rng);
            log(&[b"Sending message [", msg.as_bytes(), b" to [", &to, b"]"])?;
            tx.send(msg).unwrap();
        }
        tokio::time::sleep_until(end).await;
    }
}

async fn handle_connection(connection: Connection, rx: Receiver<String>) -> io::Result<()> {
    tokio::spawn(receiving_loop(connection.clone()));
    sending_loop(rx, connection).await
}

async fn receiving_loop(connection: Connection) -> io::Result<()> {
    let peer_addr = connection.remote_address().to_string();
    loop {
        let mut recv = connection.accept_uni().await?;
        let msg = recv.read_to_end(1024).await.unwrap();
        log(&[
            b"Received message [",
            &msg,
            b"] from ",
            peer_addr.as_bytes(),
        ])?;
    }
}

async fn sending_loop(mut rx: Receiver<String>, connection: Connection) -> Result<(), io::Error> {
    while let Ok(msg) = rx.recv().await {
        let mut send = connection.open_uni().await?;
        send.write_all(msg.as_bytes()).await?;
        send.finish().await?;
    }

    Ok(())
}
