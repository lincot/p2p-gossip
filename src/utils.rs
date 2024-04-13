use core::{
    fmt::Write,
    net::SocketAddr,
    ops::{Deref, DerefMut},
};
use std::collections::HashMap;
use tokio::sync::oneshot;

pub struct SocketAddrDeserializer<'a> {
    data: &'a [u8],
}

impl<'a> Iterator for SocketAddrDeserializer<'a> {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<Self::Item> {
        let peer: SocketAddr = bincode::deserialize(self.data).ok()?;
        self.data = &self.data[if peer.is_ipv4() {
            IPV4_SERIALIZED_LEN
        } else {
            IPV6_SERIALIZED_LEN
        }..];
        Some(peer)
    }
}

pub fn deserialize_addresses(data: &[u8]) -> SocketAddrDeserializer {
    SocketAddrDeserializer { data }
}

/// The length of a `SocketAddr::V4`, serialized with bincode.
const IPV4_SERIALIZED_LEN: usize = 10;
/// The length of a `SocketAddr::V6`, serialized with bincode.
const IPV6_SERIALIZED_LEN: usize = 22;

/// A struct holding an `oneshot::Sender` that never sends,
/// effectively allowing the thread owning the receiver
/// to await until the value is dropped.
pub struct NotifyOnDrop<T> {
    val: T,
    _tx: oneshot::Sender<()>,
}

impl<T> NotifyOnDrop<T> {
    pub fn create(val: T) -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (Self { val, _tx: tx }, rx)
    }
}

impl<T> Deref for NotifyOnDrop<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.val
    }
}

impl<T> DerefMut for NotifyOnDrop<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.val
    }
}

pub fn format_peers(peers: &HashMap<SocketAddr, bool>) -> String {
    // with IPv6, the length may be greater than the capacity provided
    let mut formatted_peers =
        String::with_capacity("\"255.255.255.255:65535\", ".len() * peers.len());
    for (i, (addr, _)) in peers
        .iter()
        .filter(|&(_, &finalized)| finalized)
        .enumerate()
    {
        if i != 0 {
            formatted_peers.push_str(", ");
        }
        write!(&mut formatted_peers, "\"{addr}\"").unwrap();
    }
    formatted_peers
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use rand::{Rng, SeedableRng};
    use rand_pcg::Pcg64Mcg;

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

    #[test]
    fn test_deserialize_addresses() {
        let mut rng = Pcg64Mcg::from_entropy();
        for _ in 0..10 {
            let len = rng.gen_range(0..100);
            let addresses: Vec<_> = (0..len)
                .map(|_| {
                    let ip = if rng.gen() {
                        IpAddr::V4(Ipv4Addr::from(rng.gen::<u32>()))
                    } else {
                        IpAddr::V6(Ipv6Addr::from(rng.gen::<u128>()))
                    };
                    SocketAddr::new(ip, rng.gen())
                })
                .collect();

            let mut data = Vec::new();
            for addr in &addresses {
                bincode::serialize_into(&mut data, addr).unwrap();
            }

            for (i, peer) in deserialize_addresses(&data).enumerate() {
                assert_eq!(peer, addresses[i]);
            }
        }
    }
}
