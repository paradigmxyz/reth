use derive_more::Deref;
use reth_trie::{
    hashed_cursor::metrics::HashedCursorMetricsCache,
    stats::{TrieStats, TrieTracker},
    trie_cursor::metrics::TrieCursorMetricsCache,
};

/// Trie stats.
#[derive(Deref, Clone, Copy, Debug)]
pub struct ParallelTrieStats {
    #[deref]
    trie: TrieStats,
    precomputed_storage_roots: u64,
    missed_leaves: u64,
}

impl ParallelTrieStats {
    /// Return general trie stats.
    pub const fn trie_stats(&self) -> TrieStats {
        self.trie
    }

    /// The number of pre-computed storage roots.
    pub const fn precomputed_storage_roots(&self) -> u64 {
        self.precomputed_storage_roots
    }

    /// The number of added leaf nodes for which we did not precompute the storage root.
    pub const fn missed_leaves(&self) -> u64 {
        self.missed_leaves
    }
}

/// Trie metrics tracker.
#[derive(Deref, Default, Debug)]
pub struct ParallelTrieTracker {
    #[deref]
    trie: TrieTracker,
    precomputed_storage_roots: u64,
    missed_leaves: u64,
    /// Metrics for account trie cursor operations.
    pub account_trie_cursor_metrics: TrieCursorMetricsCache,
    /// Metrics for account hashed cursor operations.
    pub account_hashed_cursor_metrics: HashedCursorMetricsCache,
    /// Metrics for storage trie cursor operations.
    pub storage_trie_cursor_metrics: TrieCursorMetricsCache,
    /// Metrics for storage hashed cursor operations.
    pub storage_hashed_cursor_metrics: HashedCursorMetricsCache,
}

impl ParallelTrieTracker {
    /// Set the number of precomputed storage roots.
    pub const fn set_precomputed_storage_roots(&mut self, count: u64) {
        self.precomputed_storage_roots = count;
    }

    /// Increment the number of branches added to the hash builder during the calculation.
    pub const fn inc_branch(&mut self) {
        self.trie.inc_branch();
    }

    /// Increment the number of leaves added to the hash builder during the calculation.
    pub const fn inc_leaf(&mut self) {
        self.trie.inc_leaf();
    }

    /// Increment the number of added leaf nodes for which we did not precompute the storage root.
    pub const fn inc_missed_leaves(&mut self) {
        self.missed_leaves += 1;
    }

    /// Called when root calculation is finished to return trie statistics.
    pub fn finish(self) -> ParallelTrieStats {
        ParallelTrieStats {
            trie: self.trie.finish(),
            precomputed_storage_roots: self.precomputed_storage_roots,
            missed_leaves: self.missed_leaves,
        }
    }
}
