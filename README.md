A toy QUIC P2P gossip CLI app, written in Rust.

The peer connects to all the other peers and sends them a random message
every N seconds. Received messages are printed to the stdout.

## Set up a certificate

To start a peer, obtain a certificate using a tool such as
[Certbot](https://certbot.eff.org/), or generate a self-signed certificate:

```sh
openssl req -x509 -newkey rsa:4096 -nodes -keyout key.pem -out cert.pem -days 365 -subj '/CN=localhost'
```

## Compilation

This will place the binary in `target/release/p2p-gossip`:

```sh
RUSTFLAGS="-Ctarget-cpu=native" cargo build --release
```

## Usage

```
Usage: p2p-gossip [OPTIONS] --port <PORT>

Options:
      --period <PERIOD>           Period in seconds, once in this period a random message is sent to all peers
      --ip <IP>                   IP to run on [default: 127.0.0.1]
      --port <PORT>               Port to run on
      --connect <CONNECT>         Address of the first node to connect to
      --skip-server-verification  Do not verify peers' TLS certificates
      --cert <CERT>               Path to the certificate PEM file [default: cert.pem]
      --key <KEY>                 Path to the secret key PEM file [default: key.pem]
  -h, --help                      Print help
```

## Example

```sh
./p2p-gossip --skip-server-verification --period=5 --port=8080 &>/dev/null &
./p2p-gossip --skip-server-verification --period=6 --port=8081 --connect="127.0.0.1:8080" &>/dev/null &
./p2p-gossip --skip-server-verification --period=7 --port=8082 --connect="127.0.0.1:8080"
```

prints:

```
00:00:00 - My address is "127.0.0.1:8082"
00:00:00 - Connected to the peers at ["127.0.0.1:8080", "127.0.0.1:8081"]
00:00:00 - Sending message [Fuy5qUgT5pXrWW1waxNZm5qYh5Xv1Nd2FemsWAuqx3Ci] to ["127.0.0.1:8080", "127.0.0.1:8081"]
00:00:05 - Received message [JE2ugrMxvB3YQWsavF1vXjUPwjxFuXd1C5DEiVv3p7Zt] from 127.0.0.1:8080
00:00:06 - Received message [DTgJWDQVakwuNt7aqNvSSo2of6SyaderbWBVVeFgK3HC] from 127.0.0.1:8081
00:00:07 - Sending message [8SSpE31tMdjWjkzR8XwX7EyvNb2qtqNwWdwUdPdGaiZz] to ["127.0.0.1:8080", "127.0.0.1:8081"]
00:00:10 - Received message [FZV2YiQpTWAkzBiMEKqQerGXBReUQNnnnrXqkfd7brPh] from 127.0.0.1:8080
00:00:12 - Received message [Cx9LSryYpecH1qdxHrSHKdzRWpC2D1Sw6zqcSsir2URd] from 127.0.0.1:8081
00:00:14 - Sending message [JBe7X6LzH36VPwUme24sUxLQ8P9vd8mxf17ha38qBnr6] to ["127.0.0.1:8080", "127.0.0.1:8081"]
```
