//! Reusable sparse trie cache shared by engine tree state.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_trie_sparse::SparseStateTrie;
use std::sync::Arc;
use tracing::debug;

/// Shared reusable sparse trie cache.
#[derive(Debug, Default, Clone)]
pub struct ReusableSparseTrie {
    trie: Arc<Mutex<Option<ReusableSparseTrieEntry>>>,
}

impl ReusableSparseTrie {
    /// Takes the preserved trie if present, applying continuation logic for the parent state root.
    pub fn take_for_parent(&self, parent_state_root: B256) -> Option<SparseStateTrie> {
        self.trie.lock().take().map(|entry| match entry {
            ReusableSparseTrieEntry::Anchored { trie, state_root }
                if state_root == parent_state_root =>
            {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                trie
            }
            ReusableSparseTrieEntry::Anchored { mut trie, state_root } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                trie
            }
            ReusableSparseTrieEntry::Cleared { trie } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                trie
            }
        })
    }

    /// Stores an anchored trie for later reuse.
    pub fn store_anchored(&self, trie: SparseStateTrie, state_root: B256) {
        self.trie.lock().replace(ReusableSparseTrieEntry::Anchored { trie, state_root });
    }

    /// Stores a cleared trie for allocation reuse.
    pub fn store_cleared(&self, mut trie: SparseStateTrie) {
        trie.clear();
        self.trie.lock().replace(ReusableSparseTrieEntry::Cleared { trie });
    }
}

/// A reusable sparse trie entry.
#[derive(Debug)]
enum ReusableSparseTrieEntry {
    /// Cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// Cleared sparse state trie with preserved allocations.
        trie: SparseStateTrie,
    },
    /// Trie with a computed state root that can be reused for continuation payloads.
    Anchored {
        /// The state root this trie represents.
        state_root: B256,
        /// The sparse state trie anchored to the computed state root.
        trie: SparseStateTrie,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_reuses_matching_anchored_trie() {
        let reusable = ReusableSparseTrie::default();
        let state_root = B256::with_last_byte(1);

        reusable.store_anchored(SparseStateTrie::default(), state_root);
        assert!(reusable.take_for_parent(state_root).is_some());
    }

    #[test]
    fn take_clears_mismatched_anchored_trie() {
        let reusable = ReusableSparseTrie::default();
        let ready_root = B256::with_last_byte(1);
        let parent_root = B256::with_last_byte(2);

        reusable.store_anchored(SparseStateTrie::default(), ready_root);
        let Some(trie) = reusable.take_for_parent(parent_root) else {
            panic!("anchored trie should be available")
        };

        reusable.store_anchored(trie, parent_root);
        assert!(reusable.take_for_parent(parent_root).is_some());
    }

    #[test]
    fn take_reuses_cleared_trie() {
        let reusable = ReusableSparseTrie::default();
        let parent_root = B256::with_last_byte(1);

        reusable.store_cleared(SparseStateTrie::default());
        assert!(reusable.take_for_parent(parent_root).is_some());
    }

    #[test]
    fn taking_empty_trie_allows_storing_fresh_result() {
        let reusable = ReusableSparseTrie::default();
        let parent_root = B256::with_last_byte(1);
        let result_root = B256::with_last_byte(2);

        assert!(reusable.take_for_parent(parent_root).is_none());
        reusable.store_anchored(SparseStateTrie::default(), result_root);

        assert!(reusable.take_for_parent(result_root).is_some());
    }
}
