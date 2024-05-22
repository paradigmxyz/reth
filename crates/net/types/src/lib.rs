//! Network Types and Utilities.
//!
//! This crate manages and converts Ethereum network entities such as node records, peer IDs, and
//! Ethereum Node Records (ENRs)

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_primitives::B512;
use secp256k1::{constants::UNCOMPRESSED_PUBLIC_KEY_SIZE, PublicKey, SecretKey};
use std::{net::IpAddr, str::FromStr};

// Re-export PeerId for ease of use.
pub use enr::Enr;

/// Alias for a peer identifier
pub type PeerId = B512;

pub mod node_record;
pub use node_record::{NodeRecord, NodeRecordParseError};

/// This tag should be set to indicate to libsecp256k1 that the following bytes denote an
/// uncompressed pubkey.
///
/// `SECP256K1_TAG_PUBKEY_UNCOMPRESSED` = `0x04`
///
/// See: <https://github.com/bitcoin-core/secp256k1/blob/master/include/secp256k1.h#L211>
const SECP256K1_TAG_PUBKEY_UNCOMPRESSED: u8 = 4;

/// Converts a [secp256k1::PublicKey] to a [PeerId] by stripping the
/// `SECP256K1_TAG_PUBKEY_UNCOMPRESSED` tag and storing the rest of the slice in the [PeerId].
#[inline]
pub fn pk2id(pk: &PublicKey) -> PeerId {
    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}

/// Converts a [PeerId] to a [secp256k1::PublicKey] by prepending the [PeerId] bytes with the
/// SECP256K1_TAG_PUBKEY_UNCOMPRESSED tag.
#[inline]
pub fn id2pk(id: PeerId) -> Result<PublicKey, secp256k1::Error> {
    // NOTE: B512 is used as a PeerId because 512 bits is enough to represent an uncompressed
    // public key.
    let mut s = [0u8; UNCOMPRESSED_PUBLIC_KEY_SIZE];
    s[0] = SECP256K1_TAG_PUBKEY_UNCOMPRESSED;
    s[1..].copy_from_slice(id.as_slice());
    PublicKey::from_slice(&s)
}

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
            AnyNode::Enr(enr) => pk2id(&enr.public_key()),
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
                    id: pk2id(&enr.public_key()),
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
                return Ok(AnyNode::NodeRecord(record))
            }
            // incomplete enode
            if let Ok(peer_id) = PeerId::from_str(rem) {
                return Ok(AnyNode::PeerId(peer_id))
            }
            return Err(format!("invalid public key: {rem}"))
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
                write!(f, "enode://{}", alloy_primitives::hex::encode(peer_id.as_slice()))
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
    use secp256k1::SECP256K1;

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

    #[test]
    fn pk2id2pk() {
        let prikey = SecretKey::new(&mut secp256k1::rand::thread_rng());
        let pubkey = PublicKey::from_secret_key(SECP256K1, &prikey);
        assert_eq!(pubkey, id2pk(pk2id(&pubkey)).unwrap());
    }
}
