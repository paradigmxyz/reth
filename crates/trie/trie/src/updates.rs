use crate::walker::TrieWalker;
use derive_more::Deref;
use reth_primitives::{
    trie::{
        BranchNodeCompact, HashBuilder, Nibbles, StorageTrieEntry, StoredBranchNode, StoredNibbles,
        StoredNibblesSubKey,
    },
    B256,
};
use std::collections::{hash_map::IntoIter, HashMap, HashSet};
use crate::trie_cursor::TrieCursorFactory;

/// The key of a trie node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrieKey {
    /// A node in the account trie.
    AccountNode(StoredNibbles),
    /// A node in the storage trie.
    StorageNode(B256, StoredNibblesSubKey),
    /// Storage trie of an account.
    StorageTrie(B256),
}

/// The operation to perform on the trie.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum TrieOp {
    /// Delete the node entry.
    Delete,
    /// Update the node entry with the provided value.
    Update(BranchNodeCompact),
}

impl TrieOp {
    /// Returns `true` if the operation is an update.
    pub const fn is_update(&self) -> bool {
        matches!(self, Self::Update(..))
    }
}
