//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_sparse::{
    ConfigurableSparseTrie, DeferredDrops, SparseStateTrie, SparseTrieRetainedPaths,
};
use std::sync::Arc;
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub(super) type SparseTrie = SparseStateTrie<ConfigurableSparseTrie, ConfigurableSparseTrie>;

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieCacheTask`](super::sparse_trie::SparseTrieCacheTask) for trie reuse.
#[derive(Debug, Default, Clone)]
pub(super) struct SharedPreservedSparseTrie(Arc<Mutex<Option<PreservedSparseTrie>>>);

impl SharedPreservedSparseTrie {
    /// Takes the preserved trie if present, leaving `None` in its place.
    pub(super) fn take(&self) -> Option<PreservedSparseTrie> {
        self.0.lock().take()
    }

    /// Acquires a guard that blocks `take()` until dropped.
    /// Use this before sending the state root result to ensure the next block
    /// waits for the trie to be stored.
    pub(super) fn lock(&self) -> PreservedTrieGuard<'_> {
        PreservedTrieGuard(self.0.lock())
    }

    /// Waits until the sparse trie lock becomes available.
    ///
    /// This acquires and immediately releases the lock, ensuring that any
    /// ongoing operations complete before returning. Useful for synchronization
    /// before starting payload processing.
    ///
    /// Returns the time spent waiting for the lock.
    pub(super) fn wait_for_availability(&self) -> std::time::Duration {
        let start = Instant::now();
        let _guard = self.0.lock();
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "engine::tree::payload_processor",
                blocked_for=?elapsed,
                "Waited for preserved sparse trie to become available"
            );
        }
        elapsed
    }

    /// Prunes the preserved sparse trie, retaining LFU-hot paths and the provided overlay paths.
    pub(super) fn prune(
        &self,
        max_hot_slots: usize,
        max_hot_accounts: usize,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
        retained_paths: SparseTrieRetainedPaths,
    ) -> Option<SparseTriePruneOutcome> {
        let mut guard = self.0.lock();
        let trie = guard.as_mut()?.trie_mut();
        trie.prune_with_retained_paths(max_hot_slots, max_hot_accounts, retained_paths);
        trie.shrink_to(max_nodes_capacity, max_values_capacity);
        Some(SparseTriePruneOutcome {
            memory_size: trie.memory_size(),
            retained_storage_tries: trie.retained_storage_tries_count(),
            deferred: trie.take_deferred_drops(),
        })
    }
}

/// Result of pruning a preserved sparse trie.
pub(super) struct SparseTriePruneOutcome {
    /// Deferred drops collected while the trie was preserved.
    pub(super) deferred: DeferredDrops,
    /// Memory retained by the sparse trie after pruning.
    pub(super) memory_size: usize,
    /// Storage tries retained after pruning.
    pub(super) retained_storage_tries: usize,
}

/// Guard that holds the lock on the preserved trie.
/// While held, `take()` will block. Call `store()` to save the trie before dropping.
pub(super) struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, Option<PreservedSparseTrie>>);

impl PreservedTrieGuard<'_> {
    /// Stores a preserved trie for later reuse.
    pub(super) fn store(&mut self, trie: PreservedSparseTrie) {
        self.0.replace(trie);
    }
}

/// A preserved sparse trie that can be reused across payload validations.
///
/// The trie exists in one of two states:
/// - **Anchored**: Has a computed state root and can be reused for payloads whose parent state root
///   matches the anchor.
/// - **Cleared**: Trie data has been cleared but allocations are preserved for reuse.
#[derive(Debug)]
pub(super) enum PreservedSparseTrie {
    /// Trie with a computed state root that can be reused for continuation payloads.
    Anchored {
        /// The sparse state trie anchored to the computed state root.
        trie: SparseTrie,
        /// The state root this trie represents (computed from the previous block).
        /// Used to verify continuity: new payload's `parent_state_root` must match this.
        state_root: B256,
    },
    /// Cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// The sparse state trie with cleared data but preserved allocations.
        trie: SparseTrie,
    },
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    ///
    /// The `state_root` is the computed state root from the trie, which becomes the
    /// anchor for determining if subsequent payloads can reuse this trie.
    pub(super) const fn anchored(trie: SparseTrie, state_root: B256) -> Self {
        Self::Anchored { trie, state_root }
    }

    /// Creates a cleared preserved trie (allocations preserved, data cleared).
    pub(super) const fn cleared(trie: SparseTrie) -> Self {
        Self::Cleared { trie }
    }

    /// Consumes self and returns the trie for reuse.
    ///
    /// If the preserved trie is anchored and the parent state root matches, the preserved
    /// trie structure is reused directly. Otherwise, the trie is cleared but allocations
    /// are preserved to reduce memory overhead.
    pub(super) fn into_trie_for(self, parent_state_root: B256) -> SparseTrie {
        match self {
            Self::Anchored { trie, state_root } if state_root == parent_state_root => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                trie
            }
            Self::Anchored { mut trie, state_root } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                trie
            }
            Self::Cleared { trie } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                trie
            }
        }
    }

    fn trie_mut(&mut self) -> &mut SparseTrie {
        match self {
            Self::Anchored { trie, .. } | Self::Cleared { trie } => trie,
        }
    }
}
