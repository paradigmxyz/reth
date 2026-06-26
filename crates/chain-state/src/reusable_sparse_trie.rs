//! Reusable sparse trie cache shared by engine tree state.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_sparse::SparseStateTrie;
use std::sync::Arc;
use tracing::debug;

/// Shared reusable sparse trie and the state root marker it represents.
#[derive(Debug, Default, Clone)]
pub struct ReusableSparseTrie {
    state: Arc<Mutex<ReusableSparseTrieState>>,
}

impl ReusableSparseTrie {
    /// Takes the preserved trie if present, applying continuation logic for the parent state root.
    pub fn take_for_parent(&self, parent_state_root: B256) -> Option<SparseStateTrie> {
        let mut state = self.state.lock();

        match std::mem::take(&mut *state) {
            ReusableSparseTrieState::Ready { trie, state_root }
                if state_root == parent_state_root =>
            {
                *state = ReusableSparseTrieState::Taken { parent_state_root };
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                Some(trie)
            }
            ReusableSparseTrieState::Ready { mut trie, state_root } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                *state = ReusableSparseTrieState::Taken { parent_state_root };
                Some(trie)
            }
            ReusableSparseTrieState::Cleared { trie } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                *state = ReusableSparseTrieState::Taken { parent_state_root };
                trie
            }
            taken @ ReusableSparseTrieState::Taken { parent_state_root } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %parent_state_root,
                    "Reusable sparse trie already taken"
                );
                *state = taken;
                None
            }
        }
    }

    /// Acquires a guard that blocks [`Self::take_for_parent`] until dropped.
    pub fn lock(&self) -> ReusableSparseTrieGuard<'_> {
        ReusableSparseTrieGuard { state: self.state.lock() }
    }

    /// Clears the reusable sparse trie marker and stores the trie as cleared if one is present.
    pub fn clear(&self) {
        let mut state = self.state.lock();
        let trie = match std::mem::take(&mut *state) {
            ReusableSparseTrieState::Ready { trie, .. } => Some(trie),
            ReusableSparseTrieState::Cleared { trie } => trie,
            ReusableSparseTrieState::Taken { .. } => None,
        }
        .map(|mut trie| {
            trie.clear();
            trie
        });

        *state = ReusableSparseTrieState::Cleared { trie };
    }

    /// Returns the state root represented by the reusable sparse trie.
    pub fn state_root(&self) -> Option<B256> {
        self.state.lock().state_root()
    }

    /// Sets the state root marker without installing a trie.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_state_root_for_testing(&self, state_root: B256) {
        *self.state.lock() =
            ReusableSparseTrieState::Ready { state_root, trie: SparseStateTrie::default() };
    }

    /// Waits until the reusable sparse trie lock becomes available.
    ///
    /// Returns the time spent waiting for the lock.
    pub fn wait_for_availability(&self) -> std::time::Duration {
        let start = Instant::now();
        let _guard = self.state.lock();
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "chain_state::reusable_sparse_trie",
                blocked_for=?elapsed,
                "Waited for reusable sparse trie to become available"
            );
        }
        elapsed
    }
}

/// Guard that holds the lock on the reusable sparse trie.
///
/// While held, [`ReusableSparseTrie::take_for_parent`] blocks. Store the trie before dropping the
/// guard so the next payload can reuse it.
#[derive(Debug)]
pub struct ReusableSparseTrieGuard<'a> {
    state: parking_lot::MutexGuard<'a, ReusableSparseTrieState>,
}

impl ReusableSparseTrieGuard<'_> {
    /// Stores an anchored trie for later reuse if it was not invalidated while running.
    pub fn store_anchored_if_current(
        &mut self,
        trie: SparseStateTrie,
        state_root: B256,
        taken: B256,
    ) {
        if self.state.is_taken(taken) {
            *self.state = ReusableSparseTrieState::Ready { state_root, trie };
        }
    }

    /// Stores a cleared trie for allocation reuse if it was not invalidated while running.
    pub fn store_cleared_if_current(&mut self, mut trie: SparseStateTrie, taken: B256) {
        if self.state.is_taken(taken) {
            trie.clear();
            *self.state = ReusableSparseTrieState::Cleared { trie: Some(trie) };
        }
    }
}

/// Lifecycle state for the reusable sparse trie.
#[derive(Debug)]
enum ReusableSparseTrieState {
    /// Optional cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// Cleared sparse state trie with preserved allocations.
        trie: Option<SparseStateTrie>,
    },
    /// The trie was handed to an in-flight sparse trie task.
    Taken {
        /// Parent state root this task started from.
        parent_state_root: B256,
    },
    /// Trie with a computed state root that can be reused for continuation payloads.
    Ready {
        /// The state root this trie represents.
        state_root: B256,
        /// The sparse state trie anchored to the computed state root.
        trie: SparseStateTrie,
    },
}

impl Default for ReusableSparseTrieState {
    fn default() -> Self {
        Self::Cleared { trie: None }
    }
}

impl ReusableSparseTrieState {
    fn state_root(&self) -> Option<B256> {
        match self {
            Self::Ready { state_root, .. } => Some(*state_root),
            Self::Taken { parent_state_root } => Some(*parent_state_root),
            Self::Cleared { .. } => None,
        }
    }

    fn is_taken(&self, expected: B256) -> bool {
        matches!(self, Self::Taken { parent_state_root } if *parent_state_root == expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_invalidates_in_flight_anchored_store() {
        let reusable = ReusableSparseTrie::default();
        let state_root = B256::with_last_byte(1);

        reusable.set_state_root_for_testing(state_root);
        assert_eq!(reusable.state_root(), Some(state_root));

        let Some(trie) = reusable.take_for_parent(state_root) else {
            panic!("anchored trie should be available")
        };

        reusable.clear();
        reusable.lock().store_anchored_if_current(trie, state_root, state_root);

        assert_eq!(reusable.state_root(), None);
        assert!(reusable.take_for_parent(state_root).is_none());
    }

    #[test]
    fn taking_matching_ready_trie_keeps_state_root_marker() {
        let reusable = ReusableSparseTrie::default();
        let state_root = B256::with_last_byte(1);

        reusable.set_state_root_for_testing(state_root);
        let Some(_trie) = reusable.take_for_parent(state_root) else {
            panic!("anchored trie should be available")
        };

        assert_eq!(reusable.state_root(), Some(state_root));
    }

    #[test]
    fn taking_mismatched_ready_trie_tracks_parent_root() {
        let reusable = ReusableSparseTrie::default();
        let ready_root = B256::with_last_byte(1);
        let parent_root = B256::with_last_byte(2);

        reusable.set_state_root_for_testing(ready_root);
        let Some(_trie) = reusable.take_for_parent(parent_root) else {
            panic!("cleared trie should be available")
        };

        assert_eq!(reusable.state_root(), Some(parent_root));
    }

    #[test]
    fn taking_empty_trie_allows_storing_fresh_result() {
        let reusable = ReusableSparseTrie::default();
        let parent_root = B256::with_last_byte(1);
        let result_root = B256::with_last_byte(2);

        assert!(reusable.take_for_parent(parent_root).is_none());
        reusable.lock().store_anchored_if_current(
            SparseStateTrie::default(),
            result_root,
            parent_root,
        );

        assert_eq!(reusable.state_root(), Some(result_root));
    }
}
