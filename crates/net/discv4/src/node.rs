use crate::{proto::Octets, PeerId};
use bytes::{Buf, BufMut};
use generic_array::GenericArray;
use reth_primitives::keccak256;
use reth_rlp::{Decodable, DecodeError, Encodable};
use reth_rlp_derive::RlpEncodable;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    fmt,
    fmt::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::ParseIntError,
    str::FromStr,
};
use url::{Host, Url};

/// The key type for the table.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct NodeKey(pub(crate) PeerId);

impl From<PeerId> for NodeKey {
    fn from(value: PeerId) -> Self {
        NodeKey(value)
    }
}

impl From<NodeKey> for discv5::Key<NodeKey> {
    fn from(value: NodeKey) -> Self {
        let hash = keccak256(value.0.as_bytes());
        let hash = *GenericArray::from_slice(hash.as_bytes());
        discv5::Key::new_raw(value, hash)
    }
}

/// Converts a `PeerId` into the required `Key` type for the table
#[inline]
pub(crate) fn kad_key(node: PeerId) -> discv5::Key<NodeKey> {
    discv5::kbucket::Key::from(NodeKey::from(node))
}

/// Represents a ENR in discv4.
///
/// Note: this is only an excerpt of the [ENR](enr::Enr) datastructure which is sent in Neighbours
/// message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct NodeRecord {
    /// The Address of a node.
    pub address: IpAddr,
    /// TCP port of the port that accepts connections.
    pub tcp_port: u16,
    /// UDP discovery port.
    pub udp_port: u16,
    /// Public key of the discovery service
    pub id: PeerId,
}

impl NodeRecord {
    /// Derive the [`NodeRecord`] from the secret key and addr
    pub fn from_secret_key(addr: SocketAddr, sk: &SecretKey) -> Self {
        let pk = secp256k1::PublicKey::from_secret_key(SECP256K1, sk);
        let id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
        Self::new(addr, id)
    }

    /// Creates a new record from a socket addr and peer id.
    #[allow(unused)]
    pub fn new(addr: SocketAddr, id: PeerId) -> Self {
        Self { address: addr.ip(), tcp_port: addr.port(), udp_port: addr.port(), id }
    }

    /// The TCP socket address of this node
    #[must_use]
    pub fn tcp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.tcp_port)
    }

    /// The UDP socket address of this node
    #[must_use]
    pub fn udp_addr(&self) -> SocketAddr {
        SocketAddr::new(self.address, self.udp_port)
    }

    /// Returns the key type for the kademlia table
    #[must_use]
    #[inline]
    pub(crate) fn key(&self) -> discv5::Key<NodeKey> {
        NodeKey(self.id).into()
    }
}

impl fmt::Display for NodeRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("enode://")?;
        hex::encode(self.id.as_bytes()).fmt(f)?;
        f.write_char('@')?;
        self.address.fmt(f)?;
        f.write_char(':')?;
        self.tcp_port.fmt(f)?;
        if self.tcp_port != self.udp_port {
            f.write_str("?discport=")?;
            self.udp_port.fmt(f)?;
        }

        Ok(())
    }
}

/// Possible error types when parsing a `NodeRecord`
#[derive(Debug, thiserror::Error)]
pub enum NodeRecordParseError {
    #[error("Failed to parse url: {0}")]
    InvalidUrl(String),
    #[error("Failed to parse id")]
    InvalidId(String),
    #[error("Failed to discport query: {0}")]
    Discport(ParseIntError),
}

impl FromStr for NodeRecord {
    type Err = NodeRecordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

        let udp_port = if let Some(discovery_port) =
            url.query_pairs().find_map(|(maybe_disc, port)| {
                if maybe_disc.as_ref() == "discport" {
                    Some(port)
                } else {
                    None
                }
            }) {
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

impl Encodable for NodeRecord {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(RlpEncodable)]
        struct EncodeNode {
            octets: Octets,
            udp_port: u16,
            tcp_port: u16,
            id: PeerId,
        }

        let octets = match self.address {
            IpAddr::V4(addr) => Octets::V4(addr.octets()),
            IpAddr::V6(addr) => Octets::V6(addr.octets()),
        };
        let node =
            EncodeNode { octets, udp_port: self.udp_port, tcp_port: self.tcp_port, id: self.id };
        node.encode(out)
    }
}

impl Decodable for NodeRecord {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let b = &mut &**buf;
        let rlp_head = reth_rlp::Header::decode(b)?;
        if !rlp_head.list {
            return Err(DecodeError::UnexpectedString)
        }
        let started_len = b.len();
        let octets = Octets::decode(b)?;
        let this = Self {
            address: octets.into(),
            udp_port: Decodable::decode(b)?,
            tcp_port: Decodable::decode(b)?,
            id: Decodable::decode(b)?,
        };
        // the ENR record can contain additional entries that we skip
        let consumed = started_len - b.len();
        if consumed > rlp_head.payload_length {
            return Err(DecodeError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        let rem = rlp_head.payload_length - consumed;
        b.advance(rem);
        *buf = *b;
        Ok(this)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use rand::{thread_rng, Rng, RngCore};

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
                id: PeerId::random(),
            };

            let mut buf = BytesMut::new();
            record.encode(&mut buf);

            let decoded = NodeRecord::decode(&mut buf.as_ref()).unwrap();
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
                id: PeerId::random(),
            };

            let mut buf = BytesMut::new();
            record.encode(&mut buf);

            let decoded = NodeRecord::decode(&mut buf.as_ref()).unwrap();
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
}
