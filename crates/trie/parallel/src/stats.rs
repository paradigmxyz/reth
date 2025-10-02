use derive_more::Deref;
use reth_trie::stats::{TrieStats, TrieTracker};

/// Trie stats.
#[derive(Deref, Clone, Copy, Debug)]
pub struct ParallelTrieStats {
    #[deref]
    trie: TrieStats,
    precomputed_storage_roots: u64,
    missed_leaves: u64,

    // TODO: Remove this after testing
    /// Number of storage proofs that returned immediately (already completed).
    storage_proofs_immediate: u64,
    /// Number of storage proofs that blocked waiting for completion.
    storage_proofs_blocked: u64,
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

    /// The number of storage proofs that returned immediately (already completed).
    pub const fn storage_proofs_immediate(&self) -> u64 {
        self.storage_proofs_immediate
    }

    /// The number of storage proofs that blocked waiting for completion.
    pub const fn storage_proofs_blocked(&self) -> u64 {
        self.storage_proofs_blocked
    }

    /// The percentage of storage proofs that returned immediately (0-100).
    pub fn storage_proofs_immediate_percentage(&self) -> f64 {
        let total = self.storage_proofs_immediate + self.storage_proofs_blocked;
        if total == 0 {
            0.0
        } else {
            (self.storage_proofs_immediate as f64 / total as f64) * 100.0
        }
    }
}

/// Trie metrics tracker.
#[derive(Deref, Default, Debug)]
pub struct ParallelTrieTracker {
    #[deref]
    trie: TrieTracker,
    precomputed_storage_roots: u64,
    missed_leaves: u64,
    storage_proofs_immediate: u64,
    storage_proofs_blocked: u64,
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

    /// Increment the number of storage proofs that returned immediately.
    pub const fn inc_storage_proof_immediate(&mut self) {
        self.storage_proofs_immediate += 1;
    }

    /// Increment the number of storage proofs that blocked waiting for completion.
    pub const fn inc_storage_proof_blocked(&mut self) {
        self.storage_proofs_blocked += 1;
    }

    /// Called when root calculation is finished to return trie statistics.
    pub fn finish(self) -> ParallelTrieStats {
        ParallelTrieStats {
            trie: self.trie.finish(),
            precomputed_storage_roots: self.precomputed_storage_roots,
            missed_leaves: self.missed_leaves,
            storage_proofs_immediate: self.storage_proofs_immediate,
            storage_proofs_blocked: self.storage_proofs_blocked,
        }
    }
}
