use crate::Nibbles;
use alloy_primitives::map::{B256Map, HashSet};

/// Changed trie node base paths accumulated by sparse state tries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TrieChangedPaths {
    /// Changed account trie node base paths.
    pub account_paths: HashSet<Nibbles>,
    /// Changed storage trie node base paths by hashed account address.
    pub storage_paths: B256Map<HashSet<Nibbles>>,
}

impl TrieChangedPaths {
    /// Returns true if no changed paths were retained.
    pub fn is_empty(&self) -> bool {
        self.account_paths.is_empty() && self.storage_paths.is_empty()
    }
}
