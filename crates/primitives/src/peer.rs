use enr::Enr;
use reth_rpc_types::NodeRecord;
use secp256k1::SecretKey;
use std::{net::IpAddr, str::FromStr};
// Re-export PeerId for ease of use.
pub use reth_rpc_types::PeerId;

/// A peer that can come in ENR or [NodeRecord] form.
#[derive(
    Debug, Clone, Eq, PartialEq, Hash, serde_with::SerializeDisplay, serde_with::DeserializeFromStr,
)]
pub enum AnyNode {
    /// An "enode:" peer with full ip
    NodeRecord(NodeRecord),
    /// An "enr:"
    Enr(Enr<SecretKey>),
    /// An incomplete "enode" with only a peer id
    PeerId(PeerId),
}

impl AnyNode {
    /// Returns the peer id of the node.
    pub fn peer_id(&self) -> PeerId {
        match self {
            AnyNode::NodeRecord(record) => record.id,
            AnyNode::Enr(enr) => {
                PeerId::from_slice(&enr.public_key().serialize_uncompressed()[1..])
            }
            AnyNode::PeerId(peer_id) => *peer_id,
        }
    }

    /// Returns the full node record if available.
    pub fn node_record(&self) -> Option<NodeRecord> {
        match self {
            AnyNode::NodeRecord(record) => Some(*record),
            AnyNode::Enr(enr) => {
                let node_record = NodeRecord {
                    address: enr.ip4().map(IpAddr::from).or_else(|| enr.ip6().map(IpAddr::from))?,
                    tcp_port: enr.tcp4().or_else(|| enr.tcp6())?,
                    udp_port: enr.udp4().or_else(|| enr.udp6())?,
                    id: PeerId::from_slice(&enr.public_key().serialize_uncompressed()[1..]),
                }
                .into_ipv4_mapped();
                Some(node_record)
            }
            _ => None,
        }
    }
}

impl From<NodeRecord> for AnyNode {
    fn from(value: NodeRecord) -> Self {
        Self::NodeRecord(value)
    }
}

impl From<Enr<SecretKey>> for AnyNode {
    fn from(value: Enr<SecretKey>) -> Self {
        Self::Enr(value)
    }
}

impl FromStr for AnyNode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rem) = s.strip_prefix("enode://") {
            if let Ok(record) = NodeRecord::from_str(s) {
                return Ok(AnyNode::NodeRecord(record));
            }
            // incomplete enode
            if let Ok(peer_id) = PeerId::from_str(rem) {
                return Ok(AnyNode::PeerId(peer_id));
            }
            return Err(format!("invalid public key: {rem}"));
        }
        if s.starts_with("enr:") {
            return Enr::from_str(s).map(AnyNode::Enr)
        }
        Err("missing 'enr:' prefix for base64-encoded record".to_string())
    }
}

impl std::fmt::Display for AnyNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnyNode::NodeRecord(record) => write!(f, "{record}"),
            AnyNode::Enr(enr) => write!(f, "{enr}"),
            AnyNode::PeerId(peer_id) => {
                write!(f, "enode://{}", crate::hex::encode(peer_id.as_slice()))
            }
        }
    }
}

/// Generic wrapper with peer id
#[derive(Debug)]
pub struct WithPeerId<T>(PeerId, pub T);

impl<T> From<(PeerId, T)> for WithPeerId<T> {
    fn from(value: (PeerId, T)) -> Self {
        Self(value.0, value.1)
    }
}

impl<T> WithPeerId<T> {
    /// Wraps the value with the peerid.
    pub fn new(peer: PeerId, value: T) -> Self {
        Self(peer, value)
    }

    /// Get the peer id
    pub fn peer_id(&self) -> PeerId {
        self.0
    }

    /// Get the underlying data
    pub fn data(&self) -> &T {
        &self.1
    }

    /// Returns ownership of the underlying data.
    pub fn into_data(self) -> T {
        self.1
    }

    /// Transform the data
    pub fn transform<F: From<T>>(self) -> WithPeerId<F> {
        WithPeerId(self.0, self.1.into())
    }

    /// Split the wrapper into [PeerId] and data tuple
    pub fn split(self) -> (PeerId, T) {
        (self.0, self.1)
    }

    /// Maps the inner value to a new value using the given function.
    pub fn map<U, F: FnOnce(T) -> U>(self, op: F) -> WithPeerId<U> {
        WithPeerId(self.0, op(self.1))
    }
}

impl<T> WithPeerId<Option<T>> {
    /// returns `None` if the inner value is `None`, otherwise returns `Some(WithPeerId<T>)`.
    pub fn transpose(self) -> Option<WithPeerId<T>> {
        self.1.map(|v| WithPeerId(self.0, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_record_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: AnyNode = url.parse().unwrap();
        assert_eq!(node, AnyNode::NodeRecord(NodeRecord {
            address: IpAddr::V4([10,3,58,6].into()),
            tcp_port: 30303,
            udp_port: 30301,
            id: "6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap(),
        }));
        assert_eq!(node.to_string(), url)
    }

    #[test]
    fn test_peer_id_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0";
        let node: AnyNode = url.parse().unwrap();
        assert_eq!(node, AnyNode::PeerId("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0".parse().unwrap()));
        assert_eq!(node.to_string(), url);

        let url = "enode://";
        let err = url.parse::<AnyNode>().unwrap_err();
        assert_eq!(err, "invalid public key: ");
    }

    // <https://eips.ethereum.org/EIPS/eip-778>
    #[test]
    fn test_enr_parse() {
        let url = "enr:-IS4QHCYrYZbAKWCBRlAy5zzaDZXJBGkcnh4MHcBFZntXNFrdvJjX04jRzjzCBOonrkTfj499SZuOh8R33Ls8RRcy5wBgmlkgnY0gmlwhH8AAAGJc2VjcDI1NmsxoQPKY0yuDUmstAHYpMa2_oxVtw0RW_QAdpzBQA8yWM0xOIN1ZHCCdl8";
        let node: AnyNode = url.parse().unwrap();
        assert_eq!(
            node.peer_id(),
            "0xca634cae0d49acb401d8a4c6b6fe8c55b70d115bf400769cc1400f3258cd31387574077f301b421bc84df7266c44e9e6d569fc56be00812904767bf5ccd1fc7f"
                .parse::<PeerId>()
                .unwrap()
        );
        assert_eq!(node.to_string(), url);
    }
}
