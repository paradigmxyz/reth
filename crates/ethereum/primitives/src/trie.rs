use reth_trie_common::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdatesSorted, HashedPostStateSorted,
};
use std::sync::Arc;

/// Sorted trie data computed for one executed block.
///
/// Cumulative overlays are intentionally managed by
/// [`StateTrieOverlayManager`](crate::StateTrieOverlayManager), not by each block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// Changed trie node base paths produced by state root computation.
    pub changed_paths: Option<Arc<TriePrefixSetsMut>>,
}

impl ComputedTrieData {
    /// Construct sorted trie data for one block.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self::new_with_changed_paths(hashed_state, trie_updates, None)
    }

    /// Construct sorted trie data with changed trie node base paths for one block.
    pub const fn new_with_changed_paths(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
    ) -> Self {
        Self { hashed_state, trie_updates, changed_paths }
    }
}
