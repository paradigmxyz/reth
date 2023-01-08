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
//! subtrees.    sequence-number is the tree’s update sequence number, a decimal integer.
//!    signature is a 65-byte secp256k1 EC signature over the keccak256 hash of the record content,
//! excluding the sig= part, encoded as URL-safe base64 (RFC-4648).

const ROOT_PREFIX: &str = "enrtree-root:v1";
const LINK_PREFIX: &str = "enrtree://";
const BRANCH_PREFIX: &str = "enrtree-branch:";
const ENR_PREFIX: &str = "enr:";

use bytes::Bytes;

/// Represents an `enr-root` hash of subtrees containing nodes and links.
#[derive(Clone)]
pub struct EnrTreeRootRecord {
    // base: UnsignedRoot,
    signature: Bytes,
}

// #[derive(Clone)]
// pub enum DnsRecord<K: EnrKeyUnambiguous> {
//     Root(RootRecord),
//     Link {
//         public_key: K::PublicKey,
//         domain: String,
//     },
//     Branch {
//         children: HashSet<Base32Hash>,
//     },
//     Enr {
//         record: Enr<K>,
//     },
// }