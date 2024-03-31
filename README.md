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
./p2p-gossip --skip-server-verification --period=5 --port=8080 &>/dev/null & sleep 0.1
./p2p-gossip --skip-server-verification --period=6 --port=8081 --connect="127.0.0.1:8080" &>/dev/null & sleep 0.1
./p2p-gossip --skip-server-verification --period=7 --port=8082 --connect="127.0.0.1:8080"
```

prints:

```
00:00:00 - My address is "127.0.0.1:8082"
00:00:00 - Sending message [6rMPGeRF5uVUuXtz5UBb6dorKFCZRfB2AarBt9KiQa8o] to ["127.0.0.1:8080", "127.0.0.1:8081"]
00:00:04 - Received message [BRz3XcSQy3FLu7KugEXEkspbuknEHh1VG6qGaVmGxEkm] from 127.0.0.1:8080
00:00:05 - Received message [EdtdZTZ5eBaMAAGjTxYSVGEL8eoNBpsBn11kK1AyasFK] from 127.0.0.1:8081
00:00:07 - Sending message [GZMYwhCohQYCyQaHp7mgNAMvaS46p1GD8KWxLnqh3vvU] to ["127.0.0.1:8080", "127.0.0.1:8081"]
00:00:09 - Received message [HGyi95gKHFhzRZvbZbotYzYysFqpRUD9r5Bgxvt5vt5S] from 127.0.0.1:8080
00:00:11 - Received message [9Mk3yKvyCfvpmfaxv6mNVmLbRXz8mMPZdqbvvcRqfxFJ] from 127.0.0.1:8081
00:00:14 - Sending message [FUdN353J3q8EZkgcxKba3s2R6yyn6CEr3amyAnebpH7R] to ["127.0.0.1:8080", "127.0.0.1:8081"]
```
