//! Metrics for the parallel sparse trie
use reth_metrics::{metrics::Histogram, Metrics};

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
