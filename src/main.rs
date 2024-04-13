mod config;
mod error;
mod log;
mod utils;

use backoff::ExponentialBackoff;
use clap::Parser;
use config::{configure_client_without_server_verification, read_certs_from_file};
use core::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use dns_lookup::lookup_addr;
use error::{
    is_already_open_or_locally_closed_error, is_already_open_or_locally_closed_reason, AppError,
    AppResult,
};
use futures::{future::BoxFuture, FutureExt};
use log::log;
use quinn::{ClientConfig, Connecting, Connection, ConnectionError, Endpoint, ServerConfig};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};
use tokio::{
    signal,
    sync::{broadcast, Mutex},
    time::Instant,
};
use utils::{deserialize_addresses, format_peers, NotifyOnDrop};

// this doc comment is printed at the top of the help message
/// P2P gossip peer.
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

    tokio::spawn(run_peer(endpoint.clone(), addr, args.connect, args.period));

    signal::ctrl_c().await?;
    log(&[b"Shutting down"]);
    endpoint.close(2u8.into(), b"shutdown");
    endpoint.wait_idle().await;

    Ok(())
}

/// Runs a new peer on `endpoint`.
async fn run_peer(
    endpoint: Endpoint,
    addr: SocketAddr,
    connect: Option<SocketAddr>,
    period: Option<usize>,
) {
    log(&[b"My address is \"", addr.to_string().as_bytes(), b"\""]);

    let (message_sender, _rx) = broadcast::channel::<Arc<str>>(16);

    let peers = if let Some(connect) = connect {
        initial_connect(endpoint.clone(), connect, message_sender.clone()).await
    } else {
        Arc::new(Mutex::new(HashMap::new()))
    };

    if let Some(period) = period {
        tokio::spawn(producer_loop(
            Duration::from_secs(period as _),
            peers.clone(),
            message_sender.clone(),
        ));
    }

    accept_loop(endpoint, peers, message_sender).await;
}

/// Continuesly accepts incoming connections on `Endpoint`
/// and spawns `handle_incoming_connection` on them
async fn accept_loop(
    endpoint: Endpoint,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    message_sender: broadcast::Sender<Arc<str>>,
) {
    while let Some(connecting) = endpoint.accept().await {
        tokio::spawn(handle_incoming_connection(
            endpoint.clone(),
            connecting,
            peers.clone(),
            message_sender.clone(),
        ));
    }
}

/// Accepts an incoming `connection_in_progress`.
///
/// Sends the list of peers to the remote address
/// and spawns `handle_connection`. Logs errors on failure.
async fn handle_incoming_connection(
    endpoint: Endpoint,
    connection_in_progress: Connecting,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    message_sender: broadcast::Sender<Arc<str>>,
) {
    let remote_addr = connection_in_progress.remote_address();
    match accept_connection(connection_in_progress, peers.clone()).await {
        Ok(Some(connection)) => {
            log(&[
                b"Accepted a connection from ",
                remote_addr.to_string().as_bytes(),
            ]);
            handle_connection(endpoint, connection, message_sender, peers).await;
        }
        Err(e) if !is_already_open_or_locally_closed_error(&e) => log(&[
            b"Failed to accept a connection from ",
            remote_addr.to_string().as_bytes(),
            b", error: ",
            e.to_string().as_bytes(),
        ]),
        Err(_) | Ok(None) => {}
    }
}

/// Accepts an incoming `connection_in_progress`.
///
/// Sends the list of peers to the remote address.
async fn accept_connection(
    connection_in_progress: Connecting,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
) -> AppResult<Option<Connection>> {
    let connection = connection_in_progress.await?;

    let mut peers_lock = peers.lock().await;
    if Some(true) == peers_lock.insert(connection.remote_address(), true) {
        connection.close(1u8.into(), b"already connected");
        return Ok(None);
    }

    let mut send = connection.open_uni().await?;
    for peer in &*peers_lock {
        send.write_all(&bincode::serialize(peer.0).unwrap()).await?;
    }
    drop(peers_lock);
    send.finish().await?;

    Ok(Some(connection))
}

/// Connects to `first_peer` and then to all the other peers.
async fn initial_connect(
    endpoint: Endpoint,
    first_peer: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
) -> Arc<Mutex<HashMap<SocketAddr, bool>>> {
    let peers = Arc::new(Mutex::new(HashMap::from([(first_peer, false)])));
    let (failed_peers, finished) = NotifyOnDrop::create(());
    let _ = outgoing_connect(
        endpoint,
        first_peer,
        message_sender,
        peers.clone(),
        Arc::new(failed_peers),
    )
    .await;
    let _ = finished.await;
    let mut peers_lock = peers.lock().await;
    log(&[
        b"Connected to the peers at [",
        format_peers(&peers_lock).as_bytes(),
        b"]",
    ]);
    peers_lock.retain(|_, &mut v| v);
    drop(peers_lock);
    peers
}

/// Connects to a node with address `remote_addr`. Logs errors on failure.
async fn outgoing_connect(
    endpoint: Endpoint,
    remote_addr: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    notify_on_drop: Arc<NotifyOnDrop<()>>,
) -> AppResult<Connection> {
    let local_addr = endpoint.local_addr().unwrap();
    let res = outgoing_connect_inner(
        endpoint,
        remote_addr,
        message_sender,
        peers.clone(),
        notify_on_drop.clone(),
    )
    .await;

    match res.as_ref() {
        Err(e) if !is_already_open_or_locally_closed_error(e) => log(&[
            b"Failed to connect to ",
            remote_addr.to_string().as_bytes(),
            b", error: ",
            e.to_string().as_bytes(),
        ]),
        Err(_) => {}
        Ok(connection) => {
            if Some(true) == peers.lock().await.insert(remote_addr, true)
                // a hack to avoid both ends closing the connection
                && local_addr < remote_addr
            {
                connection.close(1u8.into(), b"already connected");
            }
        }
    }

    res
}

/// Connects to a node with address `remote_addr`.
fn outgoing_connect_inner(
    endpoint: Endpoint,
    remote_addr: SocketAddr,
    message_sender: broadcast::Sender<Arc<str>>,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    failed_peers: Arc<NotifyOnDrop<()>>,
) -> BoxFuture<'static, AppResult<Connection>> {
    async move {
        let name = lookup_addr(&remote_addr.ip())?;
        let connection = endpoint.connect(remote_addr, &name)?.await?;
        let mut recv = connection.accept_uni().await?;
        let data = recv.read_to_end(10_000).await?;
        let mut peers_lock = peers.lock().await;

        for peer in deserialize_addresses(&data) {
            if peer != endpoint.local_addr().unwrap() && !peers_lock.contains_key(&peer) {
                peers_lock.insert(peer, false);
                tokio::spawn(outgoing_connect(
                    endpoint.clone(),
                    peer,
                    message_sender.clone(),
                    peers.clone(),
                    failed_peers.clone(),
                ));
            }
        }
        drop(peers_lock);
        tokio::spawn(handle_connection(
            endpoint,
            connection.clone(),
            message_sender,
            peers,
        ));
        Ok(connection)
    }
    .boxed()
}

/// Once in `duration`, sends a random message to `message_sender`.
async fn producer_loop(
    duration: Duration,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    message_sender: broadcast::Sender<Arc<str>>,
) {
    fn generate_random_message(rng: &mut impl Rng) -> String {
        let mut message = [0; 32];
        rng.fill_bytes(&mut message);
        bs58::encode(message).into_string()
    }

    let mut rng = Pcg64Mcg::from_entropy();

    let mut deadline = Instant::now() + duration;
    loop {
        tokio::time::sleep_until(deadline).await;
        deadline += duration;

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
    }
}

/// Handles communication via `connection`. Logs errors on disconnection.
async fn handle_connection(
    endpoint: Endpoint,
    connection: Connection,
    message_sender: broadcast::Sender<Arc<str>>,
    peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
) {
    async fn retry_connection(
        endpoint: Endpoint,
        remote_addr: SocketAddr,
        message_sender: broadcast::Sender<Arc<str>>,
        peers: Arc<Mutex<HashMap<SocketAddr, bool>>>,
    ) -> Result<bool, backoff::Error<AppError>> {
        if Some(&true) == peers.lock().await.get(&remote_addr) {
            return Ok(false);
        }
        let (notify_on_drop, finished) = NotifyOnDrop::create(());
        let res = outgoing_connect(
            endpoint,
            remote_addr,
            message_sender,
            peers,
            Arc::new(notify_on_drop),
        )
        .await
        .map_err(|e| backoff::Error::Transient {
            err: e,
            retry_after: None,
        });
        let _ = finished.await;
        res.map(|_| true)
    }

    let disconnect_reason = handle_connection_inner(&connection, message_sender.subscribe()).await;
    let remote_addr = connection.remote_address();

    drop(connection);
    if !is_already_open_or_locally_closed_reason(&disconnect_reason) {
        log(&[
            b"Closed connection to ",
            remote_addr.to_string().as_bytes(),
            b", reason: ",
            disconnect_reason.to_string().as_bytes(),
        ]);
    }

    peers.lock().await.insert(remote_addr, false);

    match disconnect_reason {
        ConnectionError::TimedOut => {
            // we need to reconnect even if the peer connects to us
            // to potentially get newer peers
            if backoff::future::retry(ExponentialBackoff::default(), || {
                retry_connection(
                    endpoint.clone(),
                    remote_addr,
                    message_sender.clone(),
                    peers.clone(),
                )
            })
            .await
            .unwrap()
            {
                log(&[b"Reconnected to ", remote_addr.to_string().as_bytes()]);
            }
        }
        e if is_already_open_or_locally_closed_reason(&e) => {
            peers.lock().await.insert(remote_addr, true);
        }
        _ => {}
    }
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
