//! Reusable sparse trie cache shared by engine tree state.

use alloy_primitives::B256;
use parking_lot::{Mutex, RwLock};
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_sparse::SparseStateTrie;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tracing::debug;

/// Generation token for reusable sparse trie invalidation.
pub type ReusableSparseTrieGeneration = u64;

/// Shared reusable sparse trie and the state root marker it represents.
#[derive(Debug, Default, Clone)]
pub struct ReusableSparseTrie {
    trie: Arc<Mutex<Option<PreservedSparseTrie>>>,
    state_root: Arc<RwLock<Option<B256>>>,
    generation: Arc<AtomicU64>,
}

impl ReusableSparseTrie {
    /// Takes the preserved trie if present, applying continuation logic for the parent state root.
    pub fn take_for_parent(
        &self,
        parent_state_root: B256,
    ) -> (Option<SparseStateTrie>, ReusableSparseTrieGeneration) {
        let generation = self.generation.load(Ordering::Acquire);
        let preserved = self.trie.lock().take().map(|preserved| match preserved {
            PreservedSparseTrie::Anchored { trie, state_root }
                if state_root == parent_state_root =>
            {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                trie
            }
            PreservedSparseTrie::Anchored { mut trie, state_root } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                self.clear_state_root();
                trie
            }
            PreservedSparseTrie::Cleared { trie } => {
                debug!(
                    target: "chain_state::reusable_sparse_trie",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                self.clear_state_root();
                trie
            }
        });
        (preserved, generation)
    }

    /// Acquires a guard that blocks [`Self::take_for_parent`] until dropped.
    pub fn lock(&self) -> ReusableSparseTrieGuard<'_> {
        ReusableSparseTrieGuard {
            trie: self.trie.lock(),
            state_root: Arc::clone(&self.state_root),
            generation: Arc::clone(&self.generation),
        }
    }

    /// Clears the reusable sparse trie marker and stores the trie as cleared if one is present.
    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.clear_state_root();

        let mut preserved = self.trie.lock();
        let Some(trie) = preserved.take() else { return };

        let mut trie = trie.into_cleared_trie();
        trie.clear();
        preserved.replace(PreservedSparseTrie::cleared(trie));
    }

    /// Returns the state root represented by the reusable sparse trie.
    pub fn state_root(&self) -> Option<B256> {
        *self.state_root.read()
    }

    /// Sets the state root marker without installing a trie.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_state_root_for_testing(&self, state_root: B256) {
        *self.state_root.write() = Some(state_root);
    }

    /// Waits until the reusable sparse trie lock becomes available.
    ///
    /// Returns the time spent waiting for the lock.
    pub fn wait_for_availability(&self) -> std::time::Duration {
        let start = Instant::now();
        let _guard = self.trie.lock();
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

    fn clear_state_root(&self) {
        *self.state_root.write() = None;
    }
}

/// Guard that holds the lock on the reusable sparse trie.
///
/// While held, [`ReusableSparseTrie::take_for_parent`] blocks. Store the trie before dropping the
/// guard so the next payload can reuse it.
#[derive(Debug)]
pub struct ReusableSparseTrieGuard<'a> {
    trie: parking_lot::MutexGuard<'a, Option<PreservedSparseTrie>>,
    state_root: Arc<RwLock<Option<B256>>>,
    generation: Arc<AtomicU64>,
}

impl ReusableSparseTrieGuard<'_> {
    /// Stores an anchored trie for later reuse if it was not invalidated while running.
    pub fn store_anchored_if_current(
        &mut self,
        trie: SparseStateTrie,
        state_root: B256,
        generation: ReusableSparseTrieGeneration,
    ) {
        if self.generation.load(Ordering::Acquire) == generation {
            *self.state_root.write() = Some(state_root);
            self.trie.replace(PreservedSparseTrie::anchored(trie, state_root));
        }
    }

    /// Stores a cleared trie for allocation reuse if it was not invalidated while running.
    pub fn store_cleared_if_current(
        &mut self,
        mut trie: SparseStateTrie,
        generation: ReusableSparseTrieGeneration,
    ) {
        if self.generation.load(Ordering::Acquire) == generation {
            trie.clear();
            self.trie.replace(PreservedSparseTrie::cleared(trie));
            *self.state_root.write() = None;
        }
    }
}

/// A preserved sparse trie that can be reused across payload validations.
#[derive(Debug)]
enum PreservedSparseTrie {
    /// Trie with a computed state root that can be reused for continuation payloads.
    Anchored {
        /// The sparse state trie anchored to the computed state root.
        trie: SparseStateTrie,
        /// The state root this trie represents.
        state_root: B256,
    },
    /// Cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// The sparse state trie with cleared data but preserved allocations.
        trie: SparseStateTrie,
    },
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    const fn anchored(trie: SparseStateTrie, state_root: B256) -> Self {
        Self::Anchored { trie, state_root }
    }

    /// Creates a cleared preserved trie.
    const fn cleared(trie: SparseStateTrie) -> Self {
        Self::Cleared { trie }
    }

    fn into_cleared_trie(self) -> SparseStateTrie {
        match self {
            Self::Anchored { trie, .. } | Self::Cleared { trie } => trie,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_invalidates_in_flight_anchored_store() {
        let reusable = ReusableSparseTrie::default();
        let state_root = B256::with_last_byte(1);

        let generation = reusable.generation.load(Ordering::Acquire);
        reusable.lock().store_anchored_if_current(
            SparseStateTrie::default(),
            state_root,
            generation,
        );
        assert_eq!(reusable.state_root(), Some(state_root));

        let (Some(trie), generation) = reusable.take_for_parent(state_root) else {
            panic!("anchored trie should be available")
        };

        reusable.clear();
        reusable.lock().store_anchored_if_current(trie, state_root, generation);

        assert_eq!(reusable.state_root(), None);
        assert!(reusable.take_for_parent(state_root).0.is_none());
    }
}
