//! Metrics for the sparse state trie

use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the sparse state trie
#[derive(Default, Debug)]
pub(crate) struct SparseStateTrieMetrics {
    /// Number of account nodes that were skipped during a multiproof reveal due to being redundant
    /// (i.e. they were already revealed)
    pub(crate) multiproof_skipped_account_nodes: u64,
    /// Number of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: u64,
    /// Number of storage nodes that were skipped during a multiproof reveal due to being redundant
    /// (i.e. they were already revealed)
    pub(crate) multiproof_skipped_storage_nodes: u64,
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
            .multiproof_skipped_account_nodes
            .record(take(&mut self.multiproof_skipped_account_nodes) as f64);
        self.histograms
            .multiproof_total_account_nodes
            .record(take(&mut self.multiproof_total_account_nodes) as f64);
        self.histograms
            .multiproof_skipped_storage_nodes
            .record(take(&mut self.multiproof_skipped_storage_nodes) as f64);
        self.histograms
            .multiproof_total_storage_nodes
            .record(take(&mut self.multiproof_total_storage_nodes) as f64);
    }

    /// Increment the skipped account nodes counter by the given count
    pub(crate) const fn increment_skipped_account_nodes(&mut self, count: u64) {
        self.multiproof_skipped_account_nodes += count;
    }

    /// Increment the total account nodes counter by the given count
    pub(crate) const fn increment_total_account_nodes(&mut self, count: u64) {
        self.multiproof_total_account_nodes += count;
    }

    /// Increment the skipped storage nodes counter by the given count
    pub(crate) const fn increment_skipped_storage_nodes(&mut self, count: u64) {
        self.multiproof_skipped_storage_nodes += count;
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
    /// Histogram of account nodes that were skipped during a multiproof reveal due to being
    /// redundant (i.e. they were already revealed)
    pub(crate) multiproof_skipped_account_nodes: Histogram,
    /// Histogram of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: Histogram,
    /// Histogram of storage nodes that were skipped during a multiproof reveal due to being
    /// redundant (i.e. they were already revealed)
    pub(crate) multiproof_skipped_storage_nodes: Histogram,
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
