//! Commonly used `NodeRecord` type for peers.

use crate::PeerId;
use alloc::{
    format,
    string::{String, ToString},
};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use core::{
    fmt,
    fmt::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::ParseIntError,
    str::FromStr,
};
use serde_with::{DeserializeFromStr, SerializeDisplay};

#[cfg(feature = "secp256k1")]
use enr::Enr;

/// Represents a ENR in discovery.
///
/// Note: this is only an excerpt of the [`NodeRecord`] data structure.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    Hash,
    SerializeDisplay,
    DeserializeFromStr,
    RlpEncodable,
    RlpDecodable,
)]
pub struct NodeRecord {
    /// The Address of a node.
    pub address: IpAddr,
    /// UDP discovery port.
    pub udp_port: u16,
    /// TCP port of the port that accepts connections.
    pub tcp_port: u16,
    /// Public key of the discovery service
    pub id: PeerId,
}

impl NodeRecord {
    /// Derive the [`NodeRecord`] from the secret key and addr.
    ///
    /// Note: this will set both the TCP and UDP ports to the port of the addr.
    #[cfg(feature = "secp256k1")]
    pub fn from_secret_key(addr: SocketAddr, sk: &secp256k1::SecretKey) -> Self {
        let pk = secp256k1::PublicKey::from_secret_key(secp256k1::SECP256K1, sk);
        let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
        Self::new(addr, id)
    }

    /// Converts the `address` into an [`Ipv4Addr`] if the `address` is a mapped
    /// [`Ipv6Addr`](std::net::Ipv6Addr).
    ///
    /// Returns `true` if the address was converted.
    ///
    /// See also [`std::net::Ipv6Addr::to_ipv4_mapped`]
    pub fn convert_ipv4_mapped(&mut self) -> bool {
        // convert IPv4 mapped IPv6 address
        if let IpAddr::V6(v6) = self.address {
            if let Some(v4) = v6.to_ipv4_mapped() {
                self.address = v4.into();
                return true
            }
        }
        false
    }

    /// Same as [`Self::convert_ipv4_mapped`] but consumes the type
    pub fn into_ipv4_mapped(mut self) -> Self {
        self.convert_ipv4_mapped();
        self
    }

    /// Sets the tcp port
    pub const fn with_tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = port;
        self
    }

    /// Sets the udp port
    pub const fn with_udp_port(mut self, port: u16) -> Self {
        self.udp_port = port;
        self
    }

    /// Creates a new record from a socket addr and peer id.
    pub const fn new(addr: SocketAddr, id: PeerId) -> Self {
        Self { address: addr.ip(), tcp_port: addr.port(), udp_port: addr.port(), id }
    }

    /// Creates a new record from an ip address and ports.
    pub fn new_with_ports(
        ip_addr: IpAddr,
        tcp_port: u16,
        udp_port: Option<u16>,
        id: PeerId,
    ) -> Self {
        let udp_port = udp_port.unwrap_or(tcp_port);
        Self { address: ip_addr, tcp_port, udp_port, id }
    }

    /// The TCP socket address of this node
    #[must_use]
    pub const fn tcp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.tcp_port)
    }

    /// The UDP socket address of this node
    #[must_use]
    pub const fn udp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.udp_port)
    }
}

impl fmt::Display for NodeRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("enode://")?;
        alloy_primitives::hex::encode(self.id.as_slice()).fmt(f)?;
        f.write_char('@')?;
        match self.address {
            IpAddr::V4(ip) => {
                ip.fmt(f)?;
            }
            IpAddr::V6(ip) => {
                // encapsulate with brackets
                f.write_char('[')?;
                ip.fmt(f)?;
                f.write_char(']')?;
            }
        }
        f.write_char(':')?;
        self.tcp_port.fmt(f)?;
        if self.tcp_port != self.udp_port {
            f.write_str("?discport=")?;
            self.udp_port.fmt(f)?;
        }

        Ok(())
    }
}

/// Possible error types when parsing a [`NodeRecord`]
#[derive(Debug, thiserror::Error)]
pub enum NodeRecordParseError {
    /// Invalid url
    #[error("Failed to parse url: {0}")]
    InvalidUrl(String),
    /// Invalid id
    #[error("Failed to parse id")]
    InvalidId(String),
    /// Invalid discport
    #[error("Failed to discport query: {0}")]
    Discport(ParseIntError),
}

impl FromStr for NodeRecord {
    type Err = NodeRecordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use url::{Host, Url};

        let url = Url::parse(s).map_err(|e| NodeRecordParseError::InvalidUrl(e.to_string()))?;

        let address = match url.host() {
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
            Some(Host::Domain(ip)) => IpAddr::V4(
                Ipv4Addr::from_str(ip)
                    .map_err(|e| NodeRecordParseError::InvalidUrl(e.to_string()))?,
            ),
            _ => return Err(NodeRecordParseError::InvalidUrl(format!("invalid host: {url:?}"))),
        };
        let port = url
            .port()
            .ok_or_else(|| NodeRecordParseError::InvalidUrl("no port specified".to_string()))?;

        let udp_port = if let Some(discovery_port) = url
            .query_pairs()
            .find_map(|(maybe_disc, port)| (maybe_disc.as_ref() == "discport").then_some(port))
        {
            discovery_port.parse::<u16>().map_err(NodeRecordParseError::Discport)?
        } else {
            port
        };

        let id = url
            .username()
            .parse::<PeerId>()
            .map_err(|e| NodeRecordParseError::InvalidId(e.to_string()))?;

        Ok(Self { address, id, tcp_port: port, udp_port })
    }
}

#[cfg(feature = "secp256k1")]
impl TryFrom<Enr<secp256k1::SecretKey>> for NodeRecord {
    type Error = NodeRecordParseError;

    fn try_from(enr: Enr<secp256k1::SecretKey>) -> Result<Self, Self::Error> {
        (&enr).try_into()
    }
}

#[cfg(feature = "secp256k1")]
impl TryFrom<&Enr<secp256k1::SecretKey>> for NodeRecord {
    type Error = NodeRecordParseError;

    fn try_from(enr: &Enr<secp256k1::SecretKey>) -> Result<Self, Self::Error> {
        let Some(address) = enr.ip4().map(IpAddr::from).or_else(|| enr.ip6().map(IpAddr::from))
        else {
            return Err(NodeRecordParseError::InvalidUrl("ip missing".to_string()))
        };

        let Some(udp_port) = enr.udp4().or_else(|| enr.udp6()) else {
            return Err(NodeRecordParseError::InvalidUrl("udp port missing".to_string()))
        };

        let Some(tcp_port) = enr.tcp4().or_else(|| enr.tcp6()) else {
            return Err(NodeRecordParseError::InvalidUrl("tcp port missing".to_string()))
        };

        let id = crate::pk2id(&enr.public_key());

        Ok(Self { address, tcp_port, udp_port, id }.into_ipv4_mapped())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rlp::Decodable;
    use rand::{thread_rng, Rng, RngCore};
    use std::net::Ipv6Addr;

    #[test]
    fn test_mapped_ipv6() {
        let mut rng = thread_rng();

        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        let v6 = v4.to_ipv6_mapped();

        let record = NodeRecord {
            address: v6.into(),
            tcp_port: rng.gen(),
            udp_port: rng.gen(),
            id: rng.gen(),
        };

        assert!(record.clone().convert_ipv4_mapped());
        assert_eq!(record.into_ipv4_mapped().address, IpAddr::from(v4));
    }

    #[test]
    fn test_mapped_ipv4() {
        let mut rng = thread_rng();
        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();

        let record = NodeRecord {
            address: v4.into(),
            tcp_port: rng.gen(),
            udp_port: rng.gen(),
            id: rng.gen(),
        };

        assert!(!record.clone().convert_ipv4_mapped());
        assert_eq!(record.into_ipv4_mapped().address, IpAddr::from(v4));
    }

    #[test]
    fn test_noderecord_codec_ipv4() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 4];
            rng.fill_bytes(&mut ip);
            let record = NodeRecord {
                address: IpAddr::V4(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
                id: rng.gen(),
            };

            let decoded = NodeRecord::decode(&mut alloy_rlp::encode(record).as_slice()).unwrap();
            assert_eq!(record, decoded);
        }
    }

    #[test]
    fn test_noderecord_codec_ipv6() {
        let mut rng = thread_rng();
        for _ in 0..100 {
            let mut ip = [0u8; 16];
            rng.fill_bytes(&mut ip);
            let record = NodeRecord {
                address: IpAddr::V6(ip.into()),
                tcp_port: rng.gen(),
                udp_port: rng.gen(),
                id: rng.gen(),
            };

            let decoded = NodeRecord::decode(&mut alloy_rlp::encode(record).as_slice()).unwrap();
            assert_eq!(record, decoded);
        }
    }

    #[test]
    fn test_url_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(node, NodeRecord {
            address: IpAddr::V4([10,3,58,6].into()),
            tcp_port: 30303,
            udp_port: 30301,
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
        })
    }

    #[test]
    fn test_node_display() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_display_discport() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: NodeRecord = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_serialize() {
        let cases = vec![
            // IPv4
            (
                NodeRecord {
                    address: IpAddr::V4([10, 3, 58, 6].into()),
                    tcp_port: 30303u16,
                    udp_port: 30301u16,
                    id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
                },
                "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\""
            ),
            // IPv6
            (
                NodeRecord {
                    address: Ipv6Addr::new(0x2001, 0xdb8, 0x3c4d, 0x15, 0x0, 0x0, 0xabcd, 0xef12).into(),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                },
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@[2001:db8:3c4d:15::abcd:ef12]:52150?discport=52151\"",
            )
        ];

        for (node, expected) in cases {
            let ser = serde_json::to_string::<NodeRecord>(&node).expect("couldn't serialize");
            assert_eq!(ser, expected);
        }
    }

    #[test]
    fn test_node_deserialize() {
        let cases = vec![
            // IPv4
            (
                "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\"",
                NodeRecord {
                    address: IpAddr::V4([10, 3, 58, 6].into()),
                    tcp_port: 30303u16,
                    udp_port: 30301u16,
                    id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
                }
            ),
            // IPv6
            (
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@[2001:db8:3c4d:15::abcd:ef12]:52150?discport=52151\"",
                NodeRecord {
                    address: Ipv6Addr::new(0x2001, 0xdb8, 0x3c4d, 0x15, 0x0, 0x0, 0xabcd, 0xef12).into(),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                }
            ),
        ];

        for (url, expected) in cases {
            let node: NodeRecord = serde_json::from_str(url).expect("couldn't deserialize");
            assert_eq!(node, expected);
        }
    }
}
