#![feature(write_all_vectored)]
#![feature(lazy_cell)]

mod config;
mod error;
mod log;

use clap::Parser;
use config::{configure_client_without_server_verification, read_certs_from_file};
use core::{
    fmt::Write,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use dns_lookup::lookup_addr;
use error::AppResult;
use futures::{future::BoxFuture, FutureExt};
use log::log;
use quinn::{ClientConfig, Connecting, Connection, ConnectionError, Endpoint, ServerConfig};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use std::{collections::HashSet, io, path::PathBuf, sync::Arc};
use tokio::{
    sync::{broadcast, Mutex},
    time::Instant,
};

/// Program command line arguments.
#[derive(Parser, Debug)]
struct Args {
    /// Period in seconds, once in this period a random message is sent to all peers.
    #[arg(long)]
    period: Option<usize>,
    /// IP to run on.
    #[arg(long, default_value("127.0.0.1"))]
    ip: IpAddr,
    /// Port to run on.
    #[arg(long)]
    port: u16,
    /// Address of the first node to connect to.
    #[arg(long)]
    connect: Option<SocketAddr>,
    /// Do not verify peers' TLS certificates.
    #[arg(long, action)]
    skip_server_verification: bool,
    /// Path to the certificate PEM file.
    #[arg(long, default_value("cert.pem"))]
    cert: PathBuf,
    /// Path to the secret key PEM file.
    #[arg(long, default_value("key.pem"))]
    key: PathBuf,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();
    let addr = SocketAddr::new(args.ip, args.port);

    let (certs, key) = read_certs_from_file(&args.cert, &args.key)?;
    let mut endpoint = Endpoint::server(ServerConfig::with_single_cert(certs, key).unwrap(), addr)?;
    endpoint.set_default_client_config(if args.skip_server_verification {
        configure_client_without_server_verification()
    } else {
        ClientConfig::with_native_roots()
    });

    let (message_sender, _rx) = broadcast::channel::<Arc<str>>(16);

    let peers = if let Some(connect) = args.connect {
        initial_connect(endpoint.clone(), connect, message_sender.clone()).await
    } else {
        Arc::new(Mutex::new(HashSet::new()))
    };

    if let Some(period) = args.period {
        tokio::spawn(producer_loop(
            Duration::from_secs(period as _),
            peers.clone(),
            message_sender.clone(),
        ));
    }

    log(&[b"My address is \"", addr.to_string().as_bytes(), b"\""]);

    while let Some(connecting) = endpoint.accept().await {
        tokio::spawn(handle_incoming_connection(
            connecting,
            peers.clone(),
            message_sender.subscribe(),
        ));
    }

    Ok(())
}

/// Accepts an incoming `connection_in_progress`.
///
/// Sends the list of peers to the remote address
/// and spawns `handle_connection`. Logs errors on failure.
async fn handle_incoming_connection(
    connection_in_progress: Connecting,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    message_receiver: broadcast::Receiver<Arc<str>>,
) {
    let remote_addr = connection_in_progress.remote_address();
    match accept_connection(connection_in_progress, peers.clone()).await {
        Ok(Some(connection)) => {
            log(&[
                b"Accepted a connection from ",
                remote_addr.to_string().as_bytes(),
            ]);
            handle_connection(connection, message_receiver, peers).await;
        }
        Err(e) => log(&[
            b"Failed to accept a connection from ",
            remote_addr.to_string().as_bytes(),
            b", error: ",
            e.to_string().as_bytes(),
        ]),
        Ok(None) => {}
    }
}

/// Accepts an incoming `connection_in_progress`.
///
/// Sends the list of peers to the remote address.
async fn accept_connection(
    connection_in_progress: Connecting,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
) -> AppResult<Option<Connection>> {
    let connection = connection_in_progress.await?;

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
    Ok(Some(connection))
}

/// Connects to `first_peer` and then to all the other peers.
async fn initial_connect(
    endpoint: Endpoint,
    first_peer: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
) -> Arc<Mutex<HashSet<SocketAddr>>> {
    let peers = Arc::new(Mutex::new(HashSet::from([first_peer])));
    let failed_peers = Arc::new(Mutex::new(HashSet::new()));
    outgoing_connect(
        endpoint,
        first_peer,
        message_sender,
        peers.clone(),
        failed_peers,
    )
    .await;
    peers
}

/// Connects to a node with address `addr`. Logs errors on failure.
async fn outgoing_connect(
    endpoint: Endpoint,
    addr: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    failed_peers: Arc<Mutex<HashSet<SocketAddr>>>,
) {
    if let Err(e) = outgoing_connect_inner(
        endpoint,
        addr,
        message_sender,
        peers.clone(),
        failed_peers.clone(),
    )
    .await
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

/// Connects to a node with address `addr`.
fn outgoing_connect_inner(
    endpoint: Endpoint,
    addr: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    failed_peers: Arc<Mutex<HashSet<SocketAddr>>>,
) -> BoxFuture<'static, AppResult<()>> {
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
                    message_sender.clone(),
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
        tokio::spawn(handle_connection(
            connection,
            message_sender.subscribe(),
            peers,
        ));
        Ok(())
    }
    .boxed()
}

/// Once in `duration`, sends a random message to `message_sender`.
async fn producer_loop(
    duration: Duration,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
    message_sender: broadcast::Sender<Arc<str>>,
) {
    fn generate_random_message(rng: &mut impl Rng) -> String {
        let mut message = [0; 32];
        rng.fill_bytes(&mut message);
        bs58::encode(message).into_string()
    }

    fn format_peers(peers: &HashSet<SocketAddr>) -> String {
        // with IPv6, the length may be greater than the capacity provided
        let mut formatted_peers =
            String::with_capacity("\"255.255.255.255:65535\", ".len() * peers.len());
        for (i, addr) in peers.iter().enumerate() {
            if i != 0 {
                formatted_peers.push_str(", ");
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
                formatted_peers.as_bytes(),
                b"]",
            ]);
            message_sender.send(msg.into()).unwrap();
        }
        tokio::time::sleep_until(end_time).await;
    }
}

/// Handles communication via `connection`. Logs errors on disconnection.
async fn handle_connection(
    connection: Connection,
    message_receiver: broadcast::Receiver<Arc<str>>,
    peers: Arc<Mutex<HashSet<SocketAddr>>>,
) {
    let disconnect_reason = handle_connection_inner(&connection, message_receiver).await;
    log(&[
        b"Closed connection to ",
        connection.remote_address().to_string().as_bytes(),
        b", reason: ",
        disconnect_reason.to_string().as_bytes(),
    ]);
    peers.lock().await.remove(&connection.remote_address());
}

/// Handles communication via `connection`.
async fn handle_connection_inner(
    connection: &Connection,
    mut message_receiver: broadcast::Receiver<Arc<str>>,
) -> ConnectionError {
    tokio::spawn({
        let connection = connection.clone();
        async move { sender_loop(&mut message_receiver, &connection).await }
    });
    loop {
        let receiving_res = receiver_loop(connection).await;
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

/// Logs messages received from `connection`.
async fn receiver_loop(connection: &Connection) -> AppResult<()> {
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

/// Sends messages received from `message_receiver` to `connection`.
async fn sender_loop(
    message_receiver: &mut broadcast::Receiver<Arc<str>>,
    connection: &Connection,
) -> AppResult<()> {
    while let Ok(msg) = message_receiver.recv().await {
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
    use core::net::{Ipv4Addr, Ipv6Addr};

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
