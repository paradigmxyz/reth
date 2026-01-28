//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_trie_sparse::SparseStateTrie;
use reth_trie_sparse_parallel::ParallelSparseTrie;
use std::sync::Arc;
use tracing::debug;

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieTask`](super::sparse_trie::SparseTrieTask) for trie reuse.
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

/// A preserved sparse trie with metadata about which block it was computed for.
///
/// This enables trie reuse across sequential payload validations when the new payload
/// is a direct child of the previous one.
#[derive(Debug)]
pub(super) struct PreservedSparseTrie {
    /// The sparse state trie (pruned after root computation).
    trie: SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie>,
    /// The block hash this trie was computed for.
    block_hash: B256,
}

impl PreservedSparseTrie {
    /// Creates a new preserved trie.
    pub(super) const fn new(
        trie: SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie>,
        block_hash: B256,
    ) -> Self {
        Self { trie, block_hash }
    }

    /// Returns true if this trie can be reused for a payload with the given parent hash.
    ///
    /// The trie is a continuation if the new payload's parent is the block we computed
    /// this trie for.
    fn is_continuation_of(&self, parent_hash: B256) -> bool {
        self.block_hash == parent_hash
    }

    /// Consumes self and returns the cleared trie for reuse.
    ///
    /// Currently always clears the trie because the pruned trie cannot be directly reused:
    /// - `prune()` deletes values needed for state root computation
    /// - `revealed_account_paths` is cleared, breaking proof filtering logic
    ///
    /// The allocation reuse still provides memory efficiency benefits.
    pub(super) fn into_trie_for(
        self,
        parent_hash: B256,
    ) -> SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie> {
        if self.is_continuation_of(parent_hash) {
            debug!(
                target: "engine::tree::payload_processor",
                block_hash = %self.block_hash,
                "Reusing sparse trie allocations for continuation payload"
            );
        } else {
            debug!(
                target: "engine::tree::payload_processor",
                previous_hash = %self.block_hash,
                new_parent = %parent_hash,
                "Clearing sparse trie - not a continuation"
            );
        }
        let mut trie = self.trie;
        trie.clear();
        trie
    }
}
