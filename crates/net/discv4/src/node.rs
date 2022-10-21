use crate::{proto::Octets, NodeId};
use bytes::BufMut;
use generic_array::GenericArray;
use reth_primitives::keccak256;
use reth_rlp::{Decodable, DecodeError, Encodable};
use reth_rlp_derive::{RlpDecodable, RlpEncodable};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::{Host, Url};

pub type RequestId = u64;

pub const MAX_PACKET_SIZE: usize = 1280;

pub const UPNP_INTERVAL: Duration = Duration::from_secs(60);
pub const PING_TIMEOUT: Duration = Duration::from_secs(5);
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(60);
pub const PING_INTERVAL: Duration = Duration::from_secs(10);
pub const FIND_NODE_TIMEOUT: Duration = Duration::from_secs(10);
pub const QUERY_AWAIT_PING_TIME: Duration = Duration::from_secs(2);
pub const NEIGHBOURS_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// The key type for the table.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct NodeKey(pub(crate) NodeId);

impl From<NodeId> for NodeKey {
    fn from(value: NodeId) -> Self {
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

fn ping_expiry() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + PING_TIMEOUT).as_secs()
}

fn find_node_expiry() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + FIND_NODE_TIMEOUT).as_secs()
}

/// The alpha value of discv4
pub const ALPHA: usize = 3;

/// Represents a ENR in discv4
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeRecord {
    pub address: IpAddr,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub id: NodeId,
}

impl NodeRecord {
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

/// Possible error types when parsing a `NodeRecord`
#[derive(Debug, thiserror::Error)]
pub enum NodeRecordParseError {
    #[error("Failed to parse url: {0}")]
    InvalidUrl(String),
    #[error("Failed to parse id")]
    InvalidId(String),
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
            _ => return Err(NodeRecordParseError::InvalidUrl(format!("invalid host: {:?}", url))),
        };
        let port = url
            .port()
            .ok_or_else(|| NodeRecordParseError::InvalidUrl("no port specified".to_string()))?;

        let id = url
            .username()
            .parse::<NodeId>()
            .map_err(|e| NodeRecordParseError::InvalidId(e.to_string()))?;

        Ok(Self { address, id, tcp_port: port, udp_port: port })
    }
}

impl Encodable for NodeRecord {
    fn encode(&self, out: &mut dyn BufMut) {
        let octets = match self.address {
            IpAddr::V4(addr) => Octets::V4(addr.octets()),
            IpAddr::V6(addr) => Octets::V6(addr.octets()),
        };
        let node =
            NodeOctets { octets, udp_port: self.udp_port, tcp_port: self.tcp_port, id: self.id };
        node.encode(out)
    }
}

impl Decodable for NodeRecord {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let NodeOctets { octets, udp_port, tcp_port, id } = NodeOctets::decode(buf)?;
        Ok(Self { address: octets.into(), tcp_port, udp_port, id })
    }
}

#[derive(RlpDecodable, RlpEncodable)]
struct NodeOctets {
    octets: Octets,
    udp_port: u16,
    tcp_port: u16,
    id: NodeId,
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
                id: NodeId::random(),
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
                id: NodeId::random(),
            };

            let mut buf = BytesMut::new();
            record.encode(&mut buf);

            let decoded = NodeRecord::decode(&mut buf.as_ref()).unwrap();
            assert_eq!(record, decoded);
        }
    }
}
