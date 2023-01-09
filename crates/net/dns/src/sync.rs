//! Sync trees

use crate::tree::LinkEntry;
use enr::EnrKeyUnambiguous;
use secp256k1::SecretKey;

/// A sync-able tree
pub(crate) struct SyncTree<K: EnrKeyUnambiguous = SecretKey> {
    /// The link to this tree.
    link: LinkEntry<K>,
}
