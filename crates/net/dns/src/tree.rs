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
//!    enr-root and link-root refer to the root hashes of subtrees containing nodes and links to
//! subtrees.
//!   `sequence-number` is the tree’s update sequence number, a decimal integer.
//!    `signature` is a 65-byte secp256k1 EC signature over the keccak256 hash of the record
//! content, excluding the sig= part, encoded as URL-safe base64 (RFC-4648).

use crate::error::{
    ParseDnsEntryError,
    ParseDnsEntryError::{FieldNotFound, UnknownEntry},
    ParseEntryResult,
};
use data_encoding::{BASE32_NOPAD, BASE64URL_NOPAD};
use enr::{Enr, EnrError, EnrKey, EnrKeyUnambiguous, EnrPublicKey};
use reth_primitives::{hex, Bytes};
use secp256k1::SecretKey;
#[cfg(feature = "serde")]
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};

/// Prefix used for root entries in the ENR tree.
const ROOT_V1_PREFIX: &str = "enrtree-root:v1";
/// Prefix used for link entries in the ENR tree.
const LINK_PREFIX: &str = "enrtree://";
/// Prefix used for branch entries in the ENR tree.
const BRANCH_PREFIX: &str = "enrtree-branch:";
/// Prefix used for ENR entries in the ENR tree.
const ENR_PREFIX: &str = "enr:";

/// Represents all variants of DNS entries for Ethereum node lists.
#[derive(Debug, Clone)]
pub enum DnsEntry<K: EnrKeyUnambiguous> {
    /// Represents a root entry in the DNS tree containing node records.
    Root(TreeRootEntry),
    /// Represents a link entry in the DNS tree pointing to another node list.
    Link(LinkEntry<K>),
    /// Represents a branch entry in the DNS tree containing hashes of subtree entries.
    Branch(BranchEntry),
    /// Represents a leaf entry in the DNS tree containing a node record.
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

impl<K: EnrKeyUnambiguous> FromStr for DnsEntry<K> {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(s) = s.strip_prefix(ROOT_V1_PREFIX) {
            TreeRootEntry::parse_value(s).map(DnsEntry::Root)
        } else if let Some(s) = s.strip_prefix(BRANCH_PREFIX) {
            BranchEntry::parse_value(s).map(DnsEntry::Branch)
        } else if let Some(s) = s.strip_prefix(LINK_PREFIX) {
            LinkEntry::parse_value(s).map(DnsEntry::Link)
        } else if let Some(s) = s.strip_prefix(ENR_PREFIX) {
            NodeEntry::parse_value(s).map(DnsEntry::Node)
        } else {
            Err(UnknownEntry(s.to_string()))
        }
    }
}

/// Represents an `enr-root` hash of subtrees containing nodes and links.
#[derive(Clone, Eq, PartialEq)]
pub struct TreeRootEntry {
    /// The `enr-root` hash.
    pub enr_root: String,
    /// The root hash of the links.
    pub link_root: String,
    /// The sequence number associated with the entry.
    pub sequence_number: u64,
    /// The signature of the entry.
    pub signature: Bytes,
}

// === impl TreeRootEntry ===

impl TreeRootEntry {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse_value(mut input: &str) -> ParseEntryResult<Self> {
        let input = &mut input;
        let enr_root = parse_value(input, "e=", "ENR Root", |s| Ok(s.to_string()))?;
        let link_root = parse_value(input, "l=", "Link Root", |s| Ok(s.to_string()))?;
        let sequence_number = parse_value(input, "seq=", "Sequence number", |s| {
            s.parse::<u64>().map_err(|_| {
                ParseDnsEntryError::Other(format!("Failed to parse sequence number {s}"))
            })
        })?;
        let signature = parse_value(input, "sig=", "Signature", |s| {
            BASE64URL_NOPAD.decode(s.as_bytes()).map_err(|err| {
                ParseDnsEntryError::Base64DecodeError(format!("signature error: {err}"))
            })
        })?
        .into();

        Ok(Self { enr_root, link_root, sequence_number, signature })
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

    /// Signs the content with the given key
    pub fn sign<K: EnrKey>(&mut self, key: &K) -> Result<(), EnrError> {
        let sig = key.sign_v4(self.content().as_bytes()).map_err(|_| EnrError::SigningError)?;
        self.signature = sig.into();
        Ok(())
    }

    /// Verify the signature of the record.
    #[must_use]
    pub fn verify<K: EnrKey>(&self, pubkey: &K::PublicKey) -> bool {
        let mut sig = self.signature.clone();
        sig.truncate(64);
        pubkey.verify_v4(self.content().as_bytes(), &sig)
    }
}

impl FromStr for TreeRootEntry {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(s) = s.strip_prefix(ROOT_V1_PREFIX) {
            Self::parse_value(s)
        } else {
            Err(UnknownEntry(s.to_string()))
        }
    }
}

impl fmt::Debug for TreeRootEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeRootEntry")
            .field("enr_root", &self.enr_root)
            .field("link_root", &self.link_root)
            .field("sequence_number", &self.sequence_number)
            .field("signature", &hex::encode(self.signature.as_ref()))
            .finish()
    }
}

impl fmt::Display for TreeRootEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} sig={}", self.content(), BASE64URL_NOPAD.encode(self.signature.as_ref()))
    }
}

/// Represents a branch entry in the DNS tree, containing base32 hashes of subtree entries.
#[derive(Debug, Clone)]
pub struct BranchEntry {
    /// The list of base32-encoded hashes of subtree entries in the branch.
    pub children: Vec<String>,
}

// === impl BranchEntry ===

impl BranchEntry {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse_value(input: &str) -> ParseEntryResult<Self> {
        #[inline]
        fn ensure_valid_hash(hash: &str) -> ParseEntryResult<String> {
            /// Returns the maximum length in bytes of the no-padding decoded data corresponding to
            /// `n` bytes of base32-encoded data.
            /// See also <https://cs.opensource.google/go/go/+/refs/tags/go1.19.5:src/encoding/base32/base32.go;l=526-531;drc=8a5845e4e34c046758af3729acf9221b8b6c01ae>
            #[inline(always)]
            fn base32_no_padding_decoded_len(n: usize) -> usize {
                n * 5 / 8
            }

            let decoded_len = base32_no_padding_decoded_len(hash.bytes().len());
            if !(12..=32).contains(&decoded_len) || hash.chars().any(|c| c.is_whitespace()) {
                return Err(ParseDnsEntryError::InvalidChildHash(hash.to_string()))
            }
            Ok(hash.to_string())
        }

        let children =
            input.trim().split(',').map(ensure_valid_hash).collect::<ParseEntryResult<Vec<_>>>()?;
        Ok(Self { children })
    }
}

impl FromStr for BranchEntry {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.strip_prefix(BRANCH_PREFIX)
            .map_or_else(|| Err(UnknownEntry(s.to_string())), Self::parse_value)
    }
}

impl fmt::Display for BranchEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", BRANCH_PREFIX, self.children.join(","))
    }
}

/// Represents a link entry in the DNS tree, facilitating federation and web-of-trust functionality.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(SerializeDisplay, DeserializeFromStr))]
pub struct LinkEntry<K: EnrKeyUnambiguous = SecretKey> {
    /// The domain associated with the link entry.
    pub domain: String,
    /// The public key corresponding to the Ethereum Node Record (ENR) used for the link entry.
    pub pubkey: K::PublicKey,
}

// === impl LinkEntry ===

impl<K: EnrKeyUnambiguous> LinkEntry<K> {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse_value(input: &str) -> ParseEntryResult<Self> {
        let (pubkey, domain) = input.split_once('@').ok_or_else(|| {
            ParseDnsEntryError::Other(format!("Missing @ delimiter in Link entry: {input}"))
        })?;
        let pubkey = K::decode_public(&BASE32_NOPAD.decode(pubkey.as_bytes()).map_err(|err| {
            ParseDnsEntryError::Base32DecodeError(format!("pubkey error: {err}"))
        })?)
        .map_err(|err| ParseDnsEntryError::RlpDecodeError(err.to_string()))?;

        Ok(Self { domain: domain.to_string(), pubkey })
    }
}

impl<K> PartialEq for LinkEntry<K>
where
    K: EnrKeyUnambiguous,
    K::PublicKey: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain && self.pubkey == other.pubkey
    }
}

impl<K> Eq for LinkEntry<K>
where
    K: EnrKeyUnambiguous,
    K::PublicKey: Eq + PartialEq,
{
}
impl<K> Hash for LinkEntry<K>
where
    K: EnrKeyUnambiguous,
    K::PublicKey: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.domain.hash(state);
        self.pubkey.hash(state);
    }
}

impl<K: EnrKeyUnambiguous> FromStr for LinkEntry<K> {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.strip_prefix(LINK_PREFIX)
            .map_or_else(|| Err(UnknownEntry(s.to_string())), Self::parse_value)
    }
}

impl<K: EnrKeyUnambiguous> fmt::Display for LinkEntry<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}@{}",
            LINK_PREFIX,
            BASE32_NOPAD.encode(self.pubkey.encode().as_ref()),
            self.domain
        )
    }
}

/// Represents the actual Ethereum Node Record (ENR) entry in the DNS tree.
#[derive(Debug, Clone)]
pub struct NodeEntry<K: EnrKeyUnambiguous> {
    /// The Ethereum Node Record (ENR) associated with the node entry.
    pub enr: Enr<K>,
}

// === impl NodeEntry ===

impl<K: EnrKeyUnambiguous> NodeEntry<K> {
    /// Parses the entry from text.
    ///
    /// Caution: This assumes the prefix is already removed.
    fn parse_value(s: &str) -> ParseEntryResult<Self> {
        Ok(Self { enr: s.parse().map_err(ParseDnsEntryError::Other)? })
    }
}

impl<K: EnrKeyUnambiguous> FromStr for NodeEntry<K> {
    type Err = ParseDnsEntryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.strip_prefix(ENR_PREFIX)
            .map_or_else(|| Err(UnknownEntry(s.to_string())), Self::parse_value)
    }
}

impl<K: EnrKeyUnambiguous> fmt::Display for NodeEntry<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.enr.to_base64().fmt(f)
    }
}

/// Parses the value of the key value pair
fn parse_value<F, V>(input: &mut &str, key: &str, err: &'static str, f: F) -> ParseEntryResult<V>
where
    F: Fn(&str) -> ParseEntryResult<V>,
{
    ensure_strip_key(input, key, err)?;
    let val = input.split_whitespace().next().ok_or(FieldNotFound(err))?;
    *input = &input[val.len()..];

    f(val)
}

/// Strips the `key` from the `input`
///
/// Returns an err if the `input` does not start with the `key`
fn ensure_strip_key(input: &mut &str, key: &str, err: &'static str) -> ParseEntryResult<()> {
    *input = input.trim_start().strip_prefix(key).ok_or(FieldNotFound(err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_root_entry() {
        let s = "enrtree-root:v1 e=QFT4PBCRX4XQCV3VUYJ6BTCEPU l=JGUFMSAGI7KZYB3P7IZW4S5Y3A seq=3 sig=3FmXuVwpa8Y7OstZTx9PIb1mt8FrW7VpDOFv4AaGCsZ2EIHmhraWhe4NxYhQDlw5MjeFXYMbJjsPeKlHzmJREQE";
        let root: TreeRootEntry = s.parse().unwrap();
        assert_eq!(root.to_string(), s);

        match s.parse::<DnsEntry<SecretKey>>().unwrap() {
            DnsEntry::Root(root) => {
                assert_eq!(root.to_string(), s);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_branch_entry() {
        let s = "enrtree-branch:CCCCCCCCCCCCCCCCCCCC,BBBBBBBBBBBBBBBBBBBB";
        let entry: BranchEntry = s.parse().unwrap();
        assert_eq!(entry.to_string(), s);

        match s.parse::<DnsEntry<SecretKey>>().unwrap() {
            DnsEntry::Branch(entry) => {
                assert_eq!(entry.to_string(), s);
            }
            _ => unreachable!(),
        }
    }
    #[test]
    fn parse_branch_entry_base32() {
        let s = "enrtree-branch:YNEGZIWHOM7TOOSUATAPTM";
        let entry: BranchEntry = s.parse().unwrap();
        assert_eq!(entry.to_string(), s);

        match s.parse::<DnsEntry<SecretKey>>().unwrap() {
            DnsEntry::Branch(entry) => {
                assert_eq!(entry.to_string(), s);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_invalid_branch_entry() {
        let s = "enrtree-branch:1,2";
        let res = s.parse::<BranchEntry>();
        assert!(res.is_err());
        let s = "enrtree-branch:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let res = s.parse::<BranchEntry>();
        assert!(res.is_err());

        let s = "enrtree-branch:,BBBBBBBBBBBBBBBBBBBB";
        let res = s.parse::<BranchEntry>();
        assert!(res.is_err());

        let s = "enrtree-branch:CCCCCCCCCCCCCCCCCCCC\n,BBBBBBBBBBBBBBBBBBBB";
        let res = s.parse::<BranchEntry>();
        assert!(res.is_err());
    }

    #[test]
    fn parse_link_entry() {
        let s = "enrtree://AM5FCQLWIZX2QFPNJAP7VUERCCRNGRHWZG3YYHIUV7BVDQ5FDPRT2@nodes.example.org";
        let entry: LinkEntry<SecretKey> = s.parse().unwrap();
        assert_eq!(entry.to_string(), s);

        match s.parse::<DnsEntry<SecretKey>>().unwrap() {
            DnsEntry::Link(entry) => {
                assert_eq!(entry.to_string(), s);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_enr_entry() {
        let s = "enr:-HW4QES8QIeXTYlDzbfr1WEzE-XKY4f8gJFJzjJL-9D7TC9lJb4Z3JPRRz1lP4pL_N_QpT6rGQjAU9Apnc-C1iMP36OAgmlkgnY0iXNlY3AyNTZrMaED5IdwfMxdmR8W37HqSFdQLjDkIwBd4Q_MjxgZifgKSdM";
        let entry: NodeEntry<SecretKey> = s.parse().unwrap();
        assert_eq!(entry.to_string(), s);

        match s.parse::<DnsEntry<SecretKey>>().unwrap() {
            DnsEntry::Node(entry) => {
                assert_eq!(entry.to_string(), s);
            }
            _ => unreachable!(),
        }
    }
}
