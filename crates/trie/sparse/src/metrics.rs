//! Metrics for the sparse state trie

use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the sparse state trie
#[derive(Default, Debug)]
pub(crate) struct SparseStateTrieMetrics {
    /// Number of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: u64,
    /// Number of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: u64,
    /// The actual metrics we will record into the histogram
    pub(crate) histograms: SparseStateTrieInnerMetrics,
}

impl SparseStateTrieMetrics {
    /// Record the metrics into the histograms
    pub(crate) fn record(&mut self) {
        use core::mem::take;
        self.histograms
            .multiproof_total_account_nodes
            .record(take(&mut self.multiproof_total_account_nodes) as f64);
        self.histograms
            .multiproof_total_storage_nodes
            .record(take(&mut self.multiproof_total_storage_nodes) as f64);
    }

    /// Increment the total account nodes counter by the given count
    pub(crate) const fn increment_total_account_nodes(&mut self, count: u64) {
        self.multiproof_total_account_nodes += count;
    }

    /// Increment the total storage nodes counter by the given count
    pub(crate) const fn increment_total_storage_nodes(&mut self, count: u64) {
        self.multiproof_total_storage_nodes += count;
    }
}

/// Metrics for the sparse state trie
#[derive(Metrics)]
#[metrics(scope = "sparse_state_trie")]
pub(crate) struct SparseStateTrieInnerMetrics {
    /// Histogram of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: Histogram,
    /// Histogram of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: Histogram,
}

/// Metrics for the parallel sparse trie
#[derive(Metrics, Clone)]
#[metrics(scope = "parallel_sparse_trie")]
pub(crate) struct ParallelSparseTrieMetrics {
    /// A histogram for the number of subtries updated when calculating hashes.
    pub(crate) subtries_updated: Histogram,
    /// A histogram for the time it took to update lower subtrie hashes.
    pub(crate) subtrie_hash_update_latency: Histogram,
    /// A histogram for the time it took to update the upper subtrie hashes.
    pub(crate) subtrie_upper_hash_latency: Histogram,
    /// A histogram for the time it took to reveal nodes.
    pub(crate) reveal_nodes_latency: Histogram,
    /// A histogram for the number of nodes revealed per call.
    pub(crate) num_revealed_nodes: Histogram,
    /// A histogram for the time it took to apply leaf updates.
    pub(crate) update_leaves_latency: Histogram,
    /// A histogram for the number of leaf updates per call.
    pub(crate) num_leaf_updates: Histogram,
    /// A histogram for the time it took to prune the trie.
    pub(crate) prune_latency: Histogram,
    /// A histogram for the number of nodes pruned per call.
    pub(crate) num_pruned_nodes: Histogram,
    /// A histogram for the total time to compute the root hash.
    pub(crate) root_latency: Histogram,

    // --- update_leaves phase breakdown (arena) ---
    /// Time spent sorting updates by nibbles path.
    pub(crate) update_leaves_sort_latency: Histogram,
    /// Time spent on cursor seeks + upsert/remove (excluding sort and parallel subtrie work).
    pub(crate) update_leaves_walk_latency: Histogram,
    /// Time spent on parallel subtrie leaf updates.
    pub(crate) update_leaves_parallel_latency: Histogram,
    /// Time spent on post-parallel restore/dirty-propagation pass.
    pub(crate) update_leaves_restore_latency: Histogram,
    /// Number of cursor seek calls returning RevealedLeaf.
    pub(crate) update_leaves_seek_revealed_leaf: Histogram,
    /// Number of cursor seek calls returning NoChild.
    pub(crate) update_leaves_seek_no_child: Histogram,
    /// Number of cursor seek calls returning Diverged.
    pub(crate) update_leaves_seek_diverged: Histogram,
    /// Number of cursor seek calls returning RevealedSubtrie.
    pub(crate) update_leaves_seek_subtrie: Histogram,
    /// Number of cursor seek calls returning Blinded.
    pub(crate) update_leaves_seek_blinded: Histogram,

    // --- update_leaves fast-path counters (hashmap) ---
    /// Number of updates that hit the existing-leaf fast path (no trie walk).
    pub(crate) update_leaves_existing_leaf_hits: Histogram,
    /// Number of updates that required a full trie walk.
    pub(crate) update_leaves_full_walk: Histogram,
}

impl PartialEq for ParallelSparseTrieMetrics {
    fn eq(&self, _other: &Self) -> bool {
        // It does not make sense to compare metrics, so return true, all are equal
        true
    }
}

impl Eq for ParallelSparseTrieMetrics {}
