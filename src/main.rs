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
use futures::{future::BoxFuture, FutureExt};
use log::log;
use quinn::{ClientConfig, Connecting, Connection, ConnectionError, Endpoint, ServerConfig};
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

    let (tx, _rx) = broadcast::channel::<Arc<str>>(16);

    let peers = if let Some(connect) = args.connect {
        initial_connect(endpoint.clone(), connect, tx.clone()).await
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

    log(&[b"My address is \"", addr.to_string().as_bytes(), b"\""]);

    while let Some(connecting) = endpoint.accept().await {
        let remote_addr = connecting.remote_address();
        match accept_connection(connecting, peers.clone(), tx.subscribe()).await {
            Ok(Some(_)) => log(&[
                b"Accepted a connection from ",
                remote_addr.to_string().as_bytes(),
            ]),
            Err(e) => log(&[
                b"Failed to accept a connection from ",
                remote_addr.to_string().as_bytes(),
                b", error: ",
                e.to_string().as_bytes(),
            ]),
            Ok(None) => {}
        }
    }

    Ok(())
}

async fn accept_connection(
    connecting: Connecting,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    rx: Receiver<Arc<str>>,
) -> io::Result<Option<JoinHandle<()>>> {
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
    endpoint: Endpoint,
    connect: SocketAddr,
    tx: Sender<Arc<str>>,
) -> Arc<Mutex<HashSet<SocketAddr>>> {
    let peers = Arc::new(Mutex::new(HashSet::from([connect])));
    let failed_peers = Arc::new(Mutex::new(HashSet::new()));
    outgoing_connect(endpoint, connect, tx, peers.clone(), failed_peers).await;
    peers
}

async fn outgoing_connect(
    endpoint: Endpoint,
    addr: SocketAddr,
    tx: Sender<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    failed_peers: Arc<Mutex<HashSet<SocketAddr>>>,
) {
    if let Err(e) =
        outgoing_connect_inner(endpoint, addr, tx, peers.clone(), failed_peers.clone()).await
    {
        log(&[
            b"Failed to connect to ",
            addr.to_string().as_bytes(),
            b", error: ",
            e.to_string().as_bytes(),
        ]);
        failed_peers.lock().await.insert(addr);
        peers.lock().await.remove(&addr);
    }
}

fn outgoing_connect_inner(
    endpoint: Endpoint,
    addr: SocketAddr,
    tx: Sender<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    failed_peers: Arc<Mutex<HashSet<SocketAddr>>>,
) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>> {
    async move {
        let name = lookup_addr(&addr.ip())?;
        let connection = endpoint.connect(addr, &name)?.await?;
        let mut recv = connection.accept_uni().await?;
        let data = recv.read_to_end(10_000).await?;
        let (mut peers_lock, failed_peers_lock) = (peers.lock().await, failed_peers.lock().await);
        // deserialize them one by one to avoid creating a temporary array
        let mut i = 0;
        while i + 10 <= data.len() {
            let peer = bincode::deserialize(&data[i..])?;
            if !failed_peers_lock.contains(&peer) && peers_lock.insert(peer) {
                tokio::spawn(outgoing_connect(
                    endpoint.clone(),
                    peer,
                    tx.clone(),
                    peers.clone(),
                    failed_peers.clone(),
                ));
            }
            i += if peer.is_ipv4() {
                IPV4_SERIALIZED_LEN
            } else {
                IPV6_SERIALIZED_LEN
            };
        }
        drop((peers_lock, failed_peers_lock));
        tokio::spawn(handle_connection(connection, tx.subscribe(), peers));
        Ok(())
    }
    .boxed()
}

async fn message_producing_loop(
    duration: Duration,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    tx: Sender<Arc<str>>,
) {
    fn generate_random_message(rng: &mut impl Rng) -> String {
        let mut message = [0; 32];
        rng.fill_bytes(&mut message);
        bs58::encode(message).into_string()
    }

    fn format_peers(peers: &HashSet<SocketAddr>) -> Vec<u8> {
        // with IPv6, the length may be greater than the capacity provided
        let mut formatted_peers =
            Vec::with_capacity("\"255.255.255.255:65535\", ".len() * peers.len());
        for (i, addr) in peers.iter().enumerate() {
            if i != 0 {
                formatted_peers.extend_from_slice(b", ");
            }
            write!(&mut formatted_peers, "\"{addr}\"").unwrap();
        }
        formatted_peers
    }

    let mut rng = Pcg64Mcg::from_entropy();

    loop {
        let end_time = Instant::now() + duration;
        let formatted_peers = format_peers(&*peers.lock().await);

        if !formatted_peers.is_empty() {
            let msg = generate_random_message(&mut rng);
            log(&[
                b"Sending message [",
                msg.as_bytes(),
                b"] to [",
                &formatted_peers,
                b"]",
            ]);
            tx.send(msg.into()).unwrap();
        }
        tokio::time::sleep_until(end_time).await;
    }
}

async fn handle_connection(
    connection: Connection,
    rx: Receiver<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
) {
    let disconnect_reason = handle_connection_inner(&connection, rx).await;
    log(&[
        b"Closed connection to ",
        connection.remote_address().to_string().as_bytes(),
        b", reason: ",
        disconnect_reason.to_string().as_bytes(),
    ]);
    peers.lock().await.remove(&connection.remote_address());
}

async fn handle_connection_inner(
    connection: &Connection,
    mut rx: Receiver<Arc<str>>,
) -> ConnectionError {
    tokio::spawn({
        let connection = connection.clone();
        async move { sending_loop(&mut rx, &connection).await }
    });
    loop {
        let receiving_res = receiving_loop(connection).await;
        if let Some(reason) = connection.close_reason() {
            return reason;
        }
        log(&[
            b"Failed to receive from ",
            connection.remote_address().to_string().as_bytes(),
            b", error:",
            format!("{receiving_res:?}").as_bytes(),
        ]);
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
        ]);
    }
}

async fn sending_loop(rx: &mut Receiver<Arc<str>>, connection: &Connection) -> io::Result<()> {
    while let Ok(msg) = rx.recv().await {
        let mut send = connection.open_uni().await?;
        send.write_all(msg.as_bytes()).await?;
        send.finish().await?;
    }

    Ok(())
}

/// The length of a `SocketAddr::V4`, serialized with bincode.
const IPV4_SERIALIZED_LEN: usize = 10;
/// The length of a `SocketAddr::V6`, serialized with bincode.
const IPV6_SERIALIZED_LEN: usize = 22;

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::Ipv6Addr;

    #[test]
    fn test_ipv4_serialized_len() {
        assert_eq!(
            IPV4_SERIALIZED_LEN,
            bincode::serialize(&SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                8080,
            ))
            .unwrap()
            .len()
        )
    }

    #[test]
    fn test_ipv6_serialized_len() {
        assert_eq!(
            IPV6_SERIALIZED_LEN,
            bincode::serialize(&SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc00a, 0x2ff)),
                8080,
            ))
            .unwrap()
            .len()
        );
    }
}
