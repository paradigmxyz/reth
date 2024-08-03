//! `NodeRecord` type that uses a domain instead of an IP.

use crate::{NodeRecord, PeerId};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    fmt::{self, Write},
    io::Error,
    net::IpAddr,
    num::ParseIntError,
    str::FromStr,
};
use url::Host;

/// Represents the node record of a trusted peer. The only difference between this and a
/// [`NodeRecord`] is that this does not contain the IP address of the peer, but rather a domain
/// __or__ IP address.
///
/// This is useful when specifying nodes which are in internal infrastructure and may only be
/// discoverable reliably using DNS.
///
/// This should NOT be used for any use case other than in trusted peer lists.
#[derive(Clone, Debug, Eq, PartialEq, Hash, SerializeDisplay, DeserializeFromStr)]
pub struct TrustedPeer {
    /// The host of a node.
    pub host: Host,
    /// TCP port of the port that accepts connections.
    pub tcp_port: u16,
    /// UDP discovery port.
    pub udp_port: u16,
    /// Public key of the discovery service
    pub id: PeerId,
}

impl TrustedPeer {
    /// Derive the [`NodeRecord`] from the secret key and addr
    #[cfg(feature = "secp256k1")]
    pub fn from_secret_key(host: Host, port: u16, sk: &secp256k1::SecretKey) -> Self {
        let pk = secp256k1::PublicKey::from_secret_key(secp256k1::SECP256K1, sk);
        let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
        Self::new(host, port, id)
    }

    /// Creates a new record from a socket addr and peer id.
    pub const fn new(host: Host, port: u16, id: PeerId) -> Self {
        Self { host, tcp_port: port, udp_port: port, id }
    }

    const fn to_node_record(&self, ip: IpAddr) -> NodeRecord {
        NodeRecord { address: ip, id: self.id, tcp_port: self.tcp_port, udp_port: self.udp_port }
    }

    /// Tries to resolve directly to a [`NodeRecord`] if the host is an IP address.
    fn try_node_record(&self) -> Result<NodeRecord, &str> {
        match &self.host {
            Host::Ipv4(ip) => Ok(self.to_node_record((*ip).into())),
            Host::Ipv6(ip) => Ok(self.to_node_record((*ip).into())),
            Host::Domain(domain) => Err(domain),
        }
    }

    /// Resolves the host in a [`TrustedPeer`] to an IP address, returning a [`NodeRecord`].
    ///
    /// This use [`ToSocketAddr`](std::net::ToSocketAddrs) to resolve the host to an IP address.
    pub fn resolve_blocking(&self) -> Result<NodeRecord, Error> {
        let domain = match self.try_node_record() {
            Ok(record) => return Ok(record),
            Err(domain) => domain,
        };
        // Resolve the domain to an IP address
        let mut ips = std::net::ToSocketAddrs::to_socket_addrs(&(domain, 0))?;
        let ip = ips
            .next()
            .ok_or_else(|| Error::new(std::io::ErrorKind::AddrNotAvailable, "No IP found"))?;

        Ok(self.to_node_record(ip.ip()))
    }

    /// Resolves the host in a [`TrustedPeer`] to an IP address, returning a [`NodeRecord`].
    #[cfg(any(test, feature = "net"))]
    pub async fn resolve(&self) -> Result<NodeRecord, Error> {
        let domain = match self.try_node_record() {
            Ok(record) => return Ok(record),
            Err(domain) => domain,
        };

        // Resolve the domain to an IP address
        let mut ips = tokio::net::lookup_host(format!("{domain}:0")).await?;
        let ip = ips
            .next()
            .ok_or_else(|| Error::new(std::io::ErrorKind::AddrNotAvailable, "No IP found"))?;

        Ok(self.to_node_record(ip.ip()))
    }
}

impl fmt::Display for TrustedPeer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("enode://")?;
        alloy_primitives::hex::encode(self.id.as_slice()).fmt(f)?;
        f.write_char('@')?;
        self.host.fmt(f)?;
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

impl FromStr for TrustedPeer {
    type Err = NodeRecordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use url::Url;

        // Parse the URL with enode prefix replaced with http.
        // The enode prefix causes the parser to use parse_opaque() on
        // the host str which only handles domains and ipv6, not ipv4.
        let url = Url::parse(s.replace("enode://", "http://").as_str())
            .map_err(|e| NodeRecordParseError::InvalidUrl(e.to_string()))?;

        let host = url
            .host()
            .ok_or_else(|| NodeRecordParseError::InvalidUrl("no host specified".to_string()))?
            .to_owned();

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

        Ok(Self { host, id, tcp_port: port, udp_port })
    }
}

impl From<NodeRecord> for TrustedPeer {
    fn from(record: NodeRecord) -> Self {
        let host = match record.address {
            IpAddr::V4(ip) => Host::Ipv4(ip),
            IpAddr::V6(ip) => Host::Ipv6(ip),
        };

        Self { host, tcp_port: record.tcp_port, udp_port: record.udp_port, id: record.id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn test_url_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: TrustedPeer = url.parse().unwrap();
        assert_eq!(node, TrustedPeer {
            host: Host::Ipv4([10,3,58,6].into()),
            tcp_port: 30303,
            udp_port: 30301,
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
        })
    }

    #[test]
    fn test_node_display() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303";
        let node: TrustedPeer = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_display_discport() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: TrustedPeer = url.parse().unwrap();
        assert_eq!(url, &format!("{node}"));
    }

    #[test]
    fn test_node_serialize() {
        let cases = vec![
            // IPv4
            (
                TrustedPeer {
                    host: Host::Ipv4([10, 3, 58, 6].into()),
                    tcp_port: 30303u16,
                    udp_port: 30301u16,
                    id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
                },
                "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\""
            ),
            // IPv6
            (
                TrustedPeer {
                    host: Host::Ipv6(Ipv6Addr::new(0x2001, 0xdb8, 0x3c4d, 0x15, 0x0, 0x0, 0xabcd, 0xef12)),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                },
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@[2001:db8:3c4d:15::abcd:ef12]:52150?discport=52151\""
            ),
            // URL
            (
                TrustedPeer {
                    host: Host::Domain("my-domain".to_string()),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                },
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@my-domain:52150?discport=52151\""
            ),
        ];

        for (node, expected) in cases {
            let ser = serde_json::to_string::<TrustedPeer>(&node).expect("couldn't serialize");
            assert_eq!(ser, expected);
        }
    }

    #[test]
    fn test_node_deserialize() {
        let cases = vec![
            // IPv4
            (
                "\"enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301\"",
                TrustedPeer {
                    host: Host::Ipv4([10, 3, 58, 6].into()),
                    tcp_port: 30303u16,
                    udp_port: 30301u16,
                    id: PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap(),
                }
            ),
            // IPv6
            (
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@[2001:db8:3c4d:15::abcd:ef12]:52150?discport=52151\"",
                TrustedPeer {
                    host: Host::Ipv6(Ipv6Addr::new(0x2001, 0xdb8, 0x3c4d, 0x15, 0x0, 0x0, 0xabcd, 0xef12)),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                }
            ),
            // URL
            (
                "\"enode://1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439@my-domain:52150?discport=52151\"",
                TrustedPeer {
                    host: Host::Domain("my-domain".to_string()),
                    tcp_port: 52150u16,
                    udp_port: 52151u16,
                    id: PeerId::from_str("1dd9d65c4552b5eb43d5ad55a2ee3f56c6cbc1c64a5c8d659f51fcd51bace24351232b8d7821617d2b29b54b81cdefb9b3e9c37d7fd5f63270bcc9e1a6f6a439").unwrap(),
                }
            ),
        ];

        for (url, expected) in cases {
            let node: TrustedPeer = serde_json::from_str(url).expect("couldn't deserialize");
            assert_eq!(node, expected);
        }
    }

    #[tokio::test]
    async fn test_resolve_dns_node_record() {
        // Set up tests
        let tests = vec![("localhost")];

        // Run tests
        for domain in tests {
            // Construct record
            let rec =
                TrustedPeer::new(url::Host::Domain(domain.to_owned()), 30300, PeerId::random());

            // Resolve domain and validate
            let ensure = |rec: NodeRecord| match rec.address {
                IpAddr::V4(addr) => {
                    assert_eq!(addr, std::net::Ipv4Addr::new(127, 0, 0, 1))
                }
                IpAddr::V6(addr) => {
                    assert_eq!(addr, Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))
                }
            };
            ensure(rec.resolve().await.unwrap());
            ensure(rec.resolve_blocking().unwrap());
        }
    }
}
