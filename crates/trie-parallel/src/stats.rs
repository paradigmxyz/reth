use derive_more::Deref;
use reth_trie::stats::{TrieStats, TrieTracker};

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
    pub fn trie_stats(&self) -> TrieStats {
        self.trie
    }

    /// The number of pre-computed storage roots.
    pub fn precomputed_storage_roots(&self) -> u64 {
        self.precomputed_storage_roots
    }

    /// The number of added leaf nodes for which we did not precompute the storage root.
    pub fn missed_leaves(&self) -> u64 {
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
}

impl ParallelTrieTracker {
    /// Set the number of precomputed storage roots.
    pub fn set_precomputed_storage_roots(&mut self, count: u64) {
        self.precomputed_storage_roots = count;
    }

    /// Increment the number of branches added to the hash builder during the calculation.
    pub fn inc_branch(&mut self) {
        self.trie.inc_branch();
    }

    /// Increment the number of leaves added to the hash builder during the calculation.
    pub fn inc_leaf(&mut self) {
        self.trie.inc_leaf();
    }

    /// Increment the number of added leaf nodes for which we did not precompute the storage root.
    pub fn inc_missed_leaves(&mut self) {
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
