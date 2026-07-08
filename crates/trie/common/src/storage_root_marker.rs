//! Storage trie marker node that retains only a storage root commitment for pruned account
//! storage.
//!
//! The marker is stored at the empty nibble path of an account's storage trie. Its presence
//! means the storage body (slot rows and trie nodes) was pruned while the storage root
//! commitment referenced by the account trie is retained.

use crate::BranchNodeCompact;
use alloy_primitives::B256;

/// Returns the compact storage trie marker node that retains only the storage root commitment.
pub fn node(storage_root: B256) -> BranchNodeCompact {
    BranchNodeCompact::new(0, 0, 0, Vec::new(), Some(storage_root))
}

/// Returns true if the node is a retained storage root marker.
///
/// Stored trie nodes always have a non-empty tree or hash mask, so a node with all masks empty
/// and a root hash can only be a marker written by account storage pruning. In particular, a
/// regular root node stored at the empty path also carries a root hash but has non-empty masks.
pub fn is_node(node: &BranchNodeCompact) -> bool {
    node.state_mask.is_empty() &&
        node.tree_mask.is_empty() &&
        node.hash_mask.is_empty() &&
        node.hashes.is_empty() &&
        node.root_hash.is_some()
}
