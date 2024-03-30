#![feature(write_all_vectored)]
#![feature(lazy_cell)]

// TODO: add comments

mod config;
mod log;

use clap::Parser;
use config::{configure_client_without_server_verification, read_certs_from_file};
use core::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use dns_lookup::lookup_addr;
use log::log;
use quinn::{ClientConfig, Connecting, Connection, Endpoint, ServerConfig};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use std::{
    collections::HashSet,
    error::Error,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    sync::{
        broadcast::{self, Receiver, Sender},
        Mutex,
    },
    task::JoinHandle,
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
    #[arg(long, action)]
    skip_server_verification: bool,
    #[arg(long)]
    cert: Option<PathBuf>,
    #[arg(long)]
    key: Option<PathBuf>,
    #[arg(long)]
    ip: Option<IpAddr>,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    let addr = SocketAddr::new(
        args.ip.unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        args.port,
    );

    let (certs, key) = read_certs_from_file(
        args.cert.as_ref().unwrap_or(&PathBuf::from("cert.pem")),
        args.key.as_ref().unwrap_or(&PathBuf::from("key.pem")),
    )?;
    let mut endpoint = Endpoint::server(ServerConfig::with_single_cert(certs, key).unwrap(), addr)?;
    endpoint.set_default_client_config(if args.skip_server_verification {
        configure_client_without_server_verification()
    } else {
        ClientConfig::with_native_roots()
    });

    let (tx, _rx) = broadcast::channel::<String>(16);

    let peers = if let Some(connect) = args.connect {
        initial_connect(&endpoint, connect, &tx).await?
    } else {
        Arc::new(Mutex::new(HashSet::new()))
    };

    if let Some(period) = args.period {
        tokio::spawn(message_producing_loop(
            Duration::from_secs(period as _),
            peers.clone(),
            tx.clone(),
        ));
    }

    log(&[b"My address is \"", addr.to_string().as_bytes(), b"\""])?;

    while let Some(connecting) = endpoint.accept().await {
        let remote_addr = connecting.remote_address();
        match accept_connection(connecting, peers.clone(), tx.subscribe()).await {
            Ok(Some(_)) => log(&[
                b"Accepted a connection from ",
                remote_addr.to_string().as_bytes(),
            ])?,
            Err(e) => log(&[
                b"Failed to accept a connection from ",
                remote_addr.to_string().as_bytes(),
                b", error: ",
                e.to_string().as_bytes(),
            ])?,
            Ok(None) => {}
        }
    }

    Ok(())
}

async fn accept_connection(
    connecting: Connecting,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    rx: Receiver<String>,
) -> Result<Option<JoinHandle<Result<(), io::Error>>>, Box<dyn Error>> {
    let connection = connecting.await?;

    if peers.lock().await.contains(&connection.remote_address()) {
        connection.close(1u8.into(), b"already connected");
        return Ok(None);
    }

    let mut send = connection.open_uni().await?;
    for peer in &*peers.lock().await {
        send.write_all(&bincode::serialize(peer).unwrap()).await?;
    }
    send.finish().await?;

    peers.lock().await.insert(connection.remote_address());
    Ok(Some(tokio::spawn(handle_connection(
        connection,
        rx,
        peers.clone(),
    ))))
}

async fn initial_connect(
    endpoint: &Endpoint,
    connect: SocketAddr,
    tx: &Sender<String>,
) -> io::Result<Arc<Mutex<HashSet<SocketAddr>>>> {
    // temporary empty hashmap
    let peers_arc = Arc::new(Mutex::new(HashSet::new()));
    // prevent it from being acquired while it's being filled
    let mut peers_lock = peers_arc.lock().await;
    let mut peers = HashSet::new();
    let mut failed_peers = Vec::new();

    let mut new_peers = HashSet::from([connect]);
    // it only iterates twice if the first node provides us
    // with all the others; we could as well do recursion
    while !new_peers.is_empty() {
        peers.extend(&new_peers);
        let mut newer_peers = HashSet::new();

        for addr in new_peers {
            if let Err(e) = outgoing_connect(
                endpoint,
                addr,
                tx.subscribe(),
                peers_arc.clone(),
                &peers,
                &mut newer_peers,
            )
            .await
            {
                log(&[
                    b"Failed to connect to ",
                    addr.to_string().as_bytes(),
                    b", error: ",
                    e.to_string().as_bytes(),
                ])?;
                failed_peers.push(addr);
            }
        }

        new_peers = newer_peers;
    }

    for addr in failed_peers {
        peers.remove(&addr);
    }

    *peers_lock = peers;
    drop(peers_lock);
    Ok(peers_arc)
}

async fn outgoing_connect(
    endpoint: &Endpoint,
    addr: SocketAddr,
    rx: Receiver<String>,
    peers_arc: Arc<Mutex<HashSet<SocketAddr>>>,
    peers: &HashSet<SocketAddr>,
    new_peers: &mut HashSet<SocketAddr>,
) -> Result<JoinHandle<io::Result<()>>, Box<dyn Error>> {
    let name = lookup_addr(&addr.ip())?;
    let connection = endpoint.connect(addr, &name)?.await?;
    let mut recv = connection.accept_uni().await?;
    let data = recv.read_to_end(10_000).await?;
    for segment in data.chunks_exact(10) {
        let peer = bincode::deserialize(segment)?;
        if !peers.contains(&peer) {
            new_peers.insert(peer);
        }
    }
    Ok(tokio::spawn(handle_connection(connection, rx, peers_arc)))
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
            log(&[b"Sending message [", msg.as_bytes(), b"] to [", &to, b"]"])?;
            tx.send(msg).unwrap();
        }
        tokio::time::sleep_until(end).await;
    }
}

async fn handle_connection(
    connection: Connection,
    rx: Receiver<String>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
) -> io::Result<()> {
    let res = handle_connection_inner(&connection, rx).await;
    peers.lock().await.remove(&connection.remote_address());
    res
}

async fn handle_connection_inner(
    connection: &Connection,
    mut rx: Receiver<String>,
) -> io::Result<()> {
    tokio::spawn({
        let connection = connection.clone();
        async move { sending_loop(&mut rx, &connection).await }
    });
    loop {
        let receiving_res = receiving_loop(connection).await;
        if let Some(reason) = connection.close_reason() {
            log(&[
                b"Closed connection to ",
                connection.remote_address().to_string().as_bytes(),
                b", reason: ",
                reason.to_string().as_bytes(),
            ])?;
            return Ok(());
        }
        log(&[
            b"Failed to receive from ",
            connection.remote_address().to_string().as_bytes(),
            b", error:",
            format!("{receiving_res:?}").as_bytes(),
        ])?;
    }
}

async fn receiving_loop(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let peer_addr = connection.remote_address().to_string();
    loop {
        let mut recv = connection.accept_uni().await?;
        let msg = recv.read_to_end(1024).await?;
        log(&[
            b"Received message [",
            &msg,
            b"] from ",
            peer_addr.as_bytes(),
        ])?;
    }
}

async fn sending_loop(rx: &mut Receiver<String>, connection: &Connection) -> Result<(), io::Error> {
    while let Ok(msg) = rx.recv().await {
        let mut send = connection.open_uni().await?;
        send.write_all(msg.as_bytes()).await?;
        send.finish().await?;
    }

    Ok(())
}
