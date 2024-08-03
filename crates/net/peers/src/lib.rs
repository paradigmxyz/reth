//! Network Types and Utilities.
//!
//! This crate manages and converts Ethereum network entities such as node records, peer IDs, and
//! Ethereum Node Records (ENRs)
//!
//! ## An overview of Node Record types
//!
//! Ethereum uses different types of "node records" to represent peers on the network.
//!
//! The simplest way to identify a peer is by public key. This is the [`PeerId`] type, which usually
//! represents a peer's secp256k1 public key.
//!
//! A more complete representation of a peer is the [`NodeRecord`] type, which includes the peer's
//! IP address, the ports where it is reachable (TCP and UDP), and the peer's public key. This is
//! what is returned from discovery v4 queries.
//!
//! The most comprehensive node record type is the Ethereum Node Record ([`Enr`]), which is a
//! signed, versioned record that includes the information from a [`NodeRecord`] along with
//! additional metadata. This is the data structure returned from discovery v5 queries.
//!
//! When we need to deserialize an identifier that could be any of these three types ([`PeerId`],
//! [`NodeRecord`], and [`Enr`]), we use the [`AnyNode`] type, which is an enum over the three
//! types. [`AnyNode`] is used in reth's `admin_addTrustedPeer` RPC method.
//!
//! The __final__ type is the [`TrustedPeer`] type, which is similar to a [`NodeRecord`] but may
//! include a domain name instead of a direct IP address. It includes a `resolve` method, which can
//! be used to resolve the domain name, producing a [`NodeRecord`]. This is useful for adding
//! trusted peers at startup, whose IP address may not be static each time the node starts. This is
//! common in orchestrated environments like Kubernetes, where there is reliable service discovery,
//! but services do not necessarily have static IPs.
//!
//! In short, the types are as follows:
//! - [`PeerId`]: A simple public key identifier.
//! - [`NodeRecord`]: A more complete representation of a peer, including IP address and ports.
//! - [`Enr`]: An Ethereum Node Record, which is a signed, versioned record that includes additional
//!   metadata. Useful when interacting with discovery v5, or when custom metadata is required.
//! - [`AnyNode`]: An enum over [`PeerId`], [`NodeRecord`], and [`Enr`], useful in deserialization
//!   when the type of the node record is not known.
//! - [`TrustedPeer`]: A [`NodeRecord`] with an optional domain name, which can be resolved to a
//!   [`NodeRecord`]. Useful for adding trusted peers at startup, whose IP address may not be
//!   static.
//!
//!
//! ## Feature Flags
//!
//! - `net`: Support for address lookups.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_primitives::B512;
use std::str::FromStr;

// Re-export PeerId for ease of use.
pub use enr::Enr;

/// Alias for a peer identifier
pub type PeerId = B512;

pub mod node_record;
pub use node_record::{NodeRecord, NodeRecordParseError};

pub mod trusted_peer;
pub use trusted_peer::TrustedPeer;

mod bootnodes;
pub use bootnodes::*;

/// This tag should be set to indicate to libsecp256k1 that the following bytes denote an
/// uncompressed pubkey.
///
/// `SECP256K1_TAG_PUBKEY_UNCOMPRESSED` = `0x04`
///
/// See: <https://github.com/bitcoin-core/secp256k1/blob/master/include/secp256k1.h#L211>
#[cfg(feature = "secp256k1")]
const SECP256K1_TAG_PUBKEY_UNCOMPRESSED: u8 = 4;

/// Converts a [`secp256k1::PublicKey`] to a [`PeerId`] by stripping the
/// `SECP256K1_TAG_PUBKEY_UNCOMPRESSED` tag and storing the rest of the slice in the [`PeerId`].
#[cfg(feature = "secp256k1")]
#[inline]
pub fn pk2id(pk: &secp256k1::PublicKey) -> PeerId {
    PeerId::from_slice(&pk.serialize_uncompressed()[1..])
}

/// Converts a [`PeerId`] to a [`secp256k1::PublicKey`] by prepending the [`PeerId`] bytes with the
/// `SECP256K1_TAG_PUBKEY_UNCOMPRESSED` tag.
#[cfg(feature = "secp256k1")]
#[inline]
pub fn id2pk(id: PeerId) -> Result<secp256k1::PublicKey, secp256k1::Error> {
    // NOTE: B512 is used as a PeerId because 512 bits is enough to represent an uncompressed
    // public key.
    let mut s = [0u8; secp256k1::constants::UNCOMPRESSED_PUBLIC_KEY_SIZE];
    s[0] = SECP256K1_TAG_PUBKEY_UNCOMPRESSED;
    s[1..].copy_from_slice(id.as_slice());
    secp256k1::PublicKey::from_slice(&s)
}

/// A peer that can come in ENR or [`NodeRecord`] form.
#[derive(
    Debug, Clone, Eq, PartialEq, Hash, serde_with::SerializeDisplay, serde_with::DeserializeFromStr,
)]
pub enum AnyNode {
    /// An "enode:" peer with full ip
    NodeRecord(NodeRecord),
    #[cfg(feature = "secp256k1")]
    /// An "enr:" peer
    Enr(Enr<secp256k1::SecretKey>),
    /// An incomplete "enode" with only a peer id
    PeerId(PeerId),
}

impl AnyNode {
    /// Returns the peer id of the node.
    pub fn peer_id(&self) -> PeerId {
        match self {
            Self::NodeRecord(record) => record.id,
            #[cfg(feature = "secp256k1")]
            Self::Enr(enr) => pk2id(&enr.public_key()),
            Self::PeerId(peer_id) => *peer_id,
        }
    }

    /// Returns the full node record if available.
    pub fn node_record(&self) -> Option<NodeRecord> {
        match self {
            Self::NodeRecord(record) => Some(*record),
            #[cfg(feature = "secp256k1")]
            Self::Enr(enr) => {
                let node_record = NodeRecord {
                    address: enr
                        .ip4()
                        .map(std::net::IpAddr::from)
                        .or_else(|| enr.ip6().map(std::net::IpAddr::from))?,
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

#[cfg(feature = "secp256k1")]
impl From<Enr<secp256k1::SecretKey>> for AnyNode {
    fn from(value: Enr<secp256k1::SecretKey>) -> Self {
        Self::Enr(value)
    }
}

impl FromStr for AnyNode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rem) = s.strip_prefix("enode://") {
            if let Ok(record) = NodeRecord::from_str(s) {
                return Ok(Self::NodeRecord(record))
            }
            // incomplete enode
            if let Ok(peer_id) = PeerId::from_str(rem) {
                return Ok(Self::PeerId(peer_id))
            }
            return Err(format!("invalid public key: {rem}"))
        }
        #[cfg(feature = "secp256k1")]
        if s.starts_with("enr:") {
            return Enr::from_str(s).map(AnyNode::Enr)
        }
        Err("missing 'enr:' prefix for base64-encoded record".to_string())
    }
}

impl std::fmt::Display for AnyNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeRecord(record) => write!(f, "{record}"),
            #[cfg(feature = "secp256k1")]
            Self::Enr(enr) => write!(f, "{enr}"),
            Self::PeerId(peer_id) => {
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
    pub const fn new(peer: PeerId, value: T) -> Self {
        Self(peer, value)
    }

    /// Get the peer id
    pub const fn peer_id(&self) -> PeerId {
        self.0
    }

    /// Get the underlying data
    pub const fn data(&self) -> &T {
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

    /// Split the wrapper into [`PeerId`] and data tuple
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

    #[cfg(feature = "secp256k1")]
    #[test]
    fn test_node_record_parse() {
        let url = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let node: AnyNode = url.parse().unwrap();
        assert_eq!(node, AnyNode::NodeRecord(NodeRecord {
            address: std::net::IpAddr::V4([10,3,58,6].into()),
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
    #[cfg(feature = "secp256k1")]
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
    #[cfg(feature = "secp256k1")]
    fn pk2id2pk() {
        let prikey = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pubkey = secp256k1::PublicKey::from_secret_key(secp256k1::SECP256K1, &prikey);
        assert_eq!(pubkey, id2pk(pk2id(&pubkey)).unwrap());
    }
}
