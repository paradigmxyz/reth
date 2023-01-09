//! Support for the [EIP-1459 DNS Record Structure](https://eips.ethereum.org/EIPS/eip-1459#dns-record-structure)
//!
//! The nodes in a list are encoded as a merkle tree for distribution via the DNS protocol. Entries
//! of the merkle tree are contained in DNS TXT records. The root of the tree is a TXT record with
//! the following content:
//!
//! ```text
//! enrtree-root:v1 e=<enr-root> l=<link-root> seq=<sequence-number> sig=<signature>
//! ```
//!
//! where
//!
//!    enr-root and link-root refer to the root hashes of subtrees containing nodes and links
//! subtrees.
//!   `sequence-number` is the tree’s update sequence number, a decimal integer.
//!    `signature` is a 65-byte secp256k1 EC signature over the keccak256 hash of the record
//! content, excluding the sig= part, encoded as URL-safe base64 (RFC-4648).

use crate::tree::ParseDnsEntryError::UnknownEntry;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use bytes::Bytes;
use enr::{Enr, EnrKey, EnrKeyUnambiguous, EnrPublicKey};
use std::{fmt, str::FromStr};

const ROOT_V1_PREFIX: &str = "enrtree-root:v1";
const LINK_PREFIX: &str = "enrtree://";
const BRANCH_PREFIX: &str = "enrtree-branch:";
const ENR_PREFIX: &str = "enr:";

/// Represents all variants
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub enum DnsEntry<K: EnrKeyUnambiguous> {
    Root(TreeRootEntry),
    Link(LinkEntry<K>),
    Branch(BranchEntry),
    Node(NodeEntry<K>),
}

impl<K: EnrKeyUnambiguous> fmt::Display for DnsEntry<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsEntry::Root(entry) => entry.fmt(f),
            DnsEntry::Link(entry) => entry.fmt(f),
            DnsEntry::Branch(entry) => entry.fmt(f),
            DnsEntry::Node(entry) => entry.fmt(f),
        }
    }
}

/// Error while parsing a [DnsEntry]
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ParseDnsEntryError {
    #[error("Unknown Entry: {0}")]
    UnknownEntry(String),
    #[error("{0}")]
    Other(String),
}

impl<K: EnrKeyUnambiguous> FromStr for DnsEntry<K> {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(s) = s.strip_prefix(ROOT_V1_PREFIX) {
            TreeRootEntry::parse(s).map(DnsEntry::Root)
        } else if let Some(s) = s.strip_prefix(BRANCH_PREFIX) {
            BranchEntry::parse(s).map(DnsEntry::Branch)
        } else if let Some(s) = s.strip_prefix(LINK_PREFIX) {
            LinkEntry::parse(s).map(DnsEntry::Link)
        } else if let Some(s) = s.strip_prefix(ENR_PREFIX) {
            NodeEntry::parse(s).map(DnsEntry::Node)
        } else {
            Err(UnknownEntry(s.to_string()))
        }
    }
}

/// Represents an `enr-root` hash of subtrees containing nodes and links.
#[derive(Debug, Clone)]
pub struct TreeRootEntry {
    enr_root: String,
    link_root: String,
    sequence_number: usize,
    signature: Bytes,
}

// === impl TreeRootEntry ===

impl TreeRootEntry {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse(s: &str) -> Result<Self, ParseDnsEntryError> {
        todo!()
    }

    /// Returns the _unsigned_ content pairs of the entry:
    ///
    /// ```text
    /// e=<enr-root> l=<link-root> seq=<sequence-number> sig=<signature>
    /// ```
    fn content(&self) -> String {
        format!(
            "{} e={} l={} seq={}",
            ROOT_V1_PREFIX, self.enr_root, self.link_root, self.sequence_number
        )
    }

    /// Verify the signature of the record.
    #[must_use]
    fn verify<K: EnrKey>(&self, pubkey: &K::PublicKey) -> bool {
        let mut sig = self.signature.clone();
        sig.truncate(64);
        pubkey.verify_v4(self.content().as_bytes(), &sig)
    }
}

impl fmt::Display for TreeRootEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} sig={}", self.content(), Engine::encode(&STANDARD, self.signature.as_ref()))
    }
}

/// A branch entry with base32 hashes
#[derive(Debug, Clone)]
pub struct BranchEntry {
    children: Vec<String>,
}

// === impl BranchEntry ===

impl BranchEntry {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse(s: &str) -> Result<Self, ParseDnsEntryError> {
        todo!()
    }
}

impl fmt::Display for BranchEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", BRANCH_PREFIX, self.children.join(","))
    }
}

/// A link entry
#[derive(Debug, Clone)]
pub struct LinkEntry<K: EnrKeyUnambiguous> {
    domain: String,
    pubkey: K::PublicKey,
}

// === impl LinkEntry ===

impl<K: EnrKeyUnambiguous> LinkEntry<K> {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse(s: &str) -> Result<Self, ParseDnsEntryError> {
        todo!()
    }
}

impl<K: EnrKeyUnambiguous> fmt::Display for LinkEntry<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}@{}",
            LINK_PREFIX,
            Engine::encode(&URL_SAFE_NO_PAD, self.pubkey.encode_uncompressed().as_ref()),
            self.domain
        )
    }
}

/// The actual [Enr] entry.
#[derive(Debug, Clone)]
pub struct NodeEntry<K: EnrKeyUnambiguous> {
    enr: Enr<K>,
}

// === impl LinkEntry ===

impl<K: EnrKeyUnambiguous> NodeEntry<K> {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse(s: &str) -> Result<Self, ParseDnsEntryError> {
        let enr: Enr<K> = s.parse().map_err(|msg| ParseDnsEntryError::Other(msg))?;
        Ok(Self { enr })
    }
}

impl<K: EnrKeyUnambiguous> fmt::Display for NodeEntry<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", ENR_PREFIX, self.enr.to_base64())
    }
}
