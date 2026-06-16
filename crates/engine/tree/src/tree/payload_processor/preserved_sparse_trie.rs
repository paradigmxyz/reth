//! Preserved sparse trie for reuse across payload validations.

use super::multiproof::MultiProofTaskMetrics;
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_common::HashedPostStateSorted;
use reth_trie_sparse::{ConfigurableSparseTrie, SparseStateTrie};
use std::{borrow::Cow, sync::Arc};
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub(super) type SparseTrie = SparseStateTrie<ConfigurableSparseTrie, ConfigurableSparseTrie>;

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieCacheTask`](super::sparse_trie::SparseTrieCacheTask) for trie reuse.
#[derive(Debug, Default, Clone)]
pub(super) struct SharedPreservedSparseTrie(Arc<Mutex<PreservedSparseTrieState>>);

impl SharedPreservedSparseTrie {
    /// Takes the preserved trie if present, leaving `None` in its place.
    pub(super) fn take(&self) -> Option<PreservedSparseTrie> {
        let mut state = self.0.lock();
        state.in_flight += 1;
        state.trie.take()
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

    /// Prunes persisted state keys from the preserved trie, if one is available.
    pub(super) fn prune_persisted_state(
        &self,
        persisted_block: BlockNumHash,
        persisted_state: &HashedPostStateSorted,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
    ) -> Option<std::time::Duration> {
        let mut state = self.0.lock();
        if state.in_flight > 0 {
            state.pending_prunes.push(PendingSparseTriePrune {
                persisted_block,
                persisted_state: Arc::new(persisted_state.clone()),
            });
        }

        let Some(preserved) = state.trie.as_mut() else {
            debug!(
                target: "engine::tree::payload_processor",
                "Skipping persisted sparse trie prune - no preserved trie available"
            );
            return None
        };

        let start = Instant::now();
        preserved.prune_persisted_state(
            persisted_block,
            persisted_state,
            max_nodes_capacity,
            max_values_capacity,
        );
        Some(start.elapsed())
    }
}

#[derive(Debug, Default)]
struct PreservedSparseTrieState {
    trie: Option<PreservedSparseTrie>,
    in_flight: usize,
    pending_prunes: Vec<PendingSparseTriePrune>,
}

#[derive(Debug, Clone)]
struct PendingSparseTriePrune {
    persisted_block: BlockNumHash,
    persisted_state: Arc<HashedPostStateSorted>,
}

/// Guard that holds the lock on the preserved trie.
/// While held, `take()` will block. Call `store()` to save the trie before dropping.
pub(super) struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, PreservedSparseTrieState>);

impl PreservedTrieGuard<'_> {
    /// Applies pending persistence prunes to a trie before it is stored.
    pub(super) fn apply_pending_prunes(
        &mut self,
        trie: &mut PreservedSparseTrie,
        accepted_state: Option<&HashedPostStateSorted>,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
        metrics: &MultiProofTaskMetrics,
    ) {
        for pending in &self.0.pending_prunes {
            let persisted_state = match accepted_state {
                Some(accepted_state) => Cow::Owned(HashedPostStateSorted::disjointed_merge_batch(
                    vec![pending.persisted_state.as_ref()],
                    vec![accepted_state],
                )),
                None => Cow::Borrowed(pending.persisted_state.as_ref()),
            };
            let start = Instant::now();
            trie.prune_persisted_state(
                pending.persisted_block,
                persisted_state.as_ref(),
                max_nodes_capacity,
                max_values_capacity,
            );
            metrics.sparse_trie_prune_duration_histogram.record(start.elapsed());
        }
    }

    /// Stores a preserved trie for later reuse.
    pub(super) fn store(
        &mut self,
        mut trie: PreservedSparseTrie,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
        metrics: &MultiProofTaskMetrics,
    ) {
        self.apply_pending_prunes(
            &mut trie,
            None,
            max_nodes_capacity,
            max_values_capacity,
            metrics,
        );
        self.store_pruned(trie);
    }

    /// Stores a trie that already had all pending prunes applied.
    pub(super) fn store_pruned(&mut self, trie: PreservedSparseTrie) {
        self.0.trie.replace(trie);
        self.0.in_flight = self.0.in_flight.saturating_sub(1);
        if self.0.in_flight == 0 {
            self.0.pending_prunes.clear();
        }
    }

    /// Marks an in-flight sparse trie task as discarded without replacing the preserved trie.
    pub(super) fn discard_in_flight(&mut self) {
        self.0.in_flight = self.0.in_flight.saturating_sub(1);
        if self.0.in_flight == 0 {
            self.0.pending_prunes.clear();
        }
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
        /// The sparse state trie.
        trie: SparseTrie,
        /// The state root this trie represents (computed from the previous block).
        /// Used to verify continuity: new payload's `parent_state_root` must match this.
        state_root: B256,
        /// The block that blinded children in the sparse trie fall back to.
        base_block: BlockNumHash,
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
    pub(super) const fn anchored(
        trie: SparseTrie,
        state_root: B256,
        base_block: BlockNumHash,
    ) -> Self {
        Self::Anchored { trie, state_root, base_block }
    }

    /// Creates a cleared preserved trie (allocations preserved, data cleared).
    pub(super) const fn cleared(trie: SparseTrie) -> Self {
        Self::Cleared { trie }
    }

    /// Returns the underlying sparse trie.
    pub(super) const fn trie(&self) -> &SparseTrie {
        match self {
            Self::Anchored { trie, .. } | Self::Cleared { trie } => trie,
        }
    }

    /// Returns the fallback base block for an anchored sparse trie.
    pub(super) const fn base_block(&self) -> Option<BlockNumHash> {
        match self {
            Self::Anchored { base_block, .. } => Some(*base_block),
            Self::Cleared { .. } => None,
        }
    }

    /// Consumes self and returns the trie for reuse.
    ///
    /// If the preserved trie is anchored and the parent state root matches, the pruned
    /// trie structure is reused directly. Otherwise, the trie is cleared but allocations
    /// are preserved to reduce memory overhead.
    pub(super) fn into_trie_for(
        self,
        parent: BlockNumHash,
        parent_state_root: B256,
    ) -> (SparseTrie, BlockNumHash) {
        match self {
            Self::Anchored { trie, state_root, base_block } if state_root == parent_state_root => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %state_root,
                    ?base_block,
                    "Reusing anchored sparse trie for continuation payload"
                );
                (trie, base_block)
            }
            Self::Anchored { mut trie, state_root, .. } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                (trie, parent)
            }
            Self::Cleared { trie } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    ?parent,
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                (trie, parent)
            }
        }
    }

    /// Prunes persisted state keys from this preserved trie.
    fn prune_persisted_state(
        &mut self,
        persisted_block: BlockNumHash,
        persisted_state: &HashedPostStateSorted,
        max_nodes_capacity: usize,
        max_values_capacity: usize,
    ) {
        let trie = match self {
            Self::Anchored { trie, .. } | Self::Cleared { trie } => trie,
        };

        trie.prune_persisted_state(persisted_state);
        trie.shrink_to(max_nodes_capacity, max_values_capacity);
        if let Self::Anchored { base_block, .. } = self {
            if persisted_block.number > base_block.number {
                debug!(
                    target: "engine::tree::payload_processor",
                    ?base_block,
                    ?persisted_block,
                    "Advancing sparse trie fallback base after persistence"
                );
                *base_block = persisted_block;
            } else {
                debug!(
                    target: "engine::tree::payload_processor",
                    ?base_block,
                    ?persisted_block,
                    "Keeping sparse trie fallback base after persistence"
                );
            }
        }
    }
}
