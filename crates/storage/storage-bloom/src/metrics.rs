//! Metrics for storage bloom filter.

use metrics::Counter;
use reth_metrics::Metrics;

/// Storage bloom filter metrics.
#[derive(Metrics, Clone)]
#[metrics(scope = "storage.bloom")]
pub(crate) struct StorageBloomMetrics {
    /// Number of storage reads that hit the bloom filter (definite miss, no DB read needed).
    pub bloom_hits: Counter,
    /// Number of storage reads that passed bloom filter (maybe present, need DB read).
    pub bloom_misses: Counter,
    /// Number of storage reads that were false positives (bloom said maybe, but DB returned empty).
    pub bloom_false_positives: Counter,
    /// Number of storage slots inserted into bloom filter.
    pub bloom_inserts: Counter,
}
