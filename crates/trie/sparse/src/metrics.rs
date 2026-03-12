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
}

impl PartialEq for ParallelSparseTrieMetrics {
    fn eq(&self, _other: &Self) -> bool {
        // It does not make sense to compare metrics, so return true, all are equal
        true
    }
}

impl Eq for ParallelSparseTrieMetrics {}

/// Metrics for the arena-based parallel sparse trie `update_leaves`.
#[derive(Metrics, Clone)]
#[metrics(scope = "arena_sparse_trie")]
pub(crate) struct ArenaUpdateLeavesMetrics {
    /// Total time spent in `update_leaves`.
    pub(crate) update_leaves_total_latency: Histogram,
    /// Time spent draining the updates HashMap and sorting by nibbles path.
    pub(crate) update_leaves_drain_sort_latency: Histogram,
    /// Time spent in upper-trie walk (seek + dispatch, excluding subtrie updates).
    pub(crate) update_leaves_upper_walk_latency: Histogram,
    /// Time spent doing inline (serial) subtrie updates.
    pub(crate) update_leaves_inline_subtrie_latency: Histogram,
    /// Time spent doing parallel subtrie updates (wall-clock).
    pub(crate) update_leaves_parallel_subtrie_latency: Histogram,
    /// Time spent in post-parallel dirty-state propagation walk.
    pub(crate) update_leaves_post_parallel_latency: Histogram,

    /// Number of leaf updates processed.
    pub(crate) update_leaves_num_updates: Histogram,
    /// Number of subtries updated inline (below parallelism threshold).
    pub(crate) update_leaves_inline_subtrie_count: Histogram,
    /// Number of subtries taken for parallel update.
    pub(crate) update_leaves_parallel_subtrie_count: Histogram,
    /// Number of upper-trie seeks in the main walk loop.
    pub(crate) update_leaves_upper_seek_count: Histogram,

    // --- Subtrie-level (ArenaSparseSubtrie::update_leaves) ---

    /// Time spent in cursor.seek across all iterations within a single subtrie update_leaves.
    pub(crate) subtrie_update_leaves_seek_latency: Histogram,
    /// Time spent in upsert_leaf calls within a single subtrie update_leaves.
    pub(crate) subtrie_update_leaves_upsert_latency: Histogram,
    /// Time spent in remove_leaf calls within a single subtrie update_leaves.
    pub(crate) subtrie_update_leaves_remove_latency: Histogram,
    /// Time spent in cursor.drain at the end of a subtrie update_leaves.
    pub(crate) subtrie_update_leaves_drain_latency: Histogram,
    /// Number of seeks performed in a subtrie update_leaves call.
    pub(crate) subtrie_update_leaves_seek_count: Histogram,
}

impl PartialEq for ArenaUpdateLeavesMetrics {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ArenaUpdateLeavesMetrics {}
