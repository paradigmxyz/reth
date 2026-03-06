//! Metrics for the payload builder impl

use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};

/// Payload builder metrics
#[derive(Metrics)]
#[metrics(scope = "payloads")]
pub struct PayloadBuilderMetrics {
    /// Total number of times an empty payload was returned because a built one was not ready.
    pub(crate) requested_empty_payload: Counter,
    /// Total number of initiated payload build attempts.
    pub(crate) initiated_payload_builds: Counter,
    /// Total number of failed payload build attempts.
    pub(crate) failed_payload_builds: Counter,
    /// Time spent on transaction selection and validation within the execution loop.
    pub(crate) tx_selection_duration_seconds: Histogram,
    /// Time spent on EVM transaction execution within the execution loop.
    pub(crate) evm_execution_duration_seconds: Histogram,
    /// Number of account read cache hits during payload building.
    pub(crate) state_cache_account_hits: Counter,
    /// Number of account read cache misses during payload building.
    pub(crate) state_cache_account_misses: Counter,
    /// Number of storage read cache hits during payload building.
    pub(crate) state_cache_storage_hits: Counter,
    /// Number of storage read cache misses during payload building.
    pub(crate) state_cache_storage_misses: Counter,
    /// Number of code read cache hits during payload building.
    pub(crate) state_cache_code_hits: Counter,
    /// Number of code read cache misses during payload building.
    pub(crate) state_cache_code_misses: Counter,
    /// Number of block hash read cache hits during payload building.
    pub(crate) state_cache_block_hash_hits: Counter,
    /// Number of block hash read cache misses during payload building.
    pub(crate) state_cache_block_hash_misses: Counter,
}

impl PayloadBuilderMetrics {
    pub(crate) fn inc_requested_empty_payload(&self) {
        self.requested_empty_payload.increment(1);
    }

    pub(crate) fn inc_initiated_payload_builds(&self) {
        self.initiated_payload_builds.increment(1);
    }

    pub(crate) fn inc_failed_payload_builds(&self) {
        self.failed_payload_builds.increment(1);
    }

    /// Records the cumulative time spent on transaction selection and validation.
    pub fn record_tx_selection_duration(&self, duration_secs: f64) {
        self.tx_selection_duration_seconds.record(duration_secs);
    }

    /// Records the cumulative time spent on EVM transaction execution.
    pub fn record_evm_execution_duration(&self, duration_secs: f64) {
        self.evm_execution_duration_seconds.record(duration_secs);
    }

    /// Records cache hit/miss statistics from a payload build pass.
    pub fn record_cache_stats(&self, stats: &reth_revm::cached::CacheStats) {
        self.state_cache_account_hits.increment(stats.account_hits);
        self.state_cache_account_misses.increment(stats.account_misses);
        self.state_cache_storage_hits.increment(stats.storage_hits);
        self.state_cache_storage_misses.increment(stats.storage_misses);
        self.state_cache_code_hits.increment(stats.code_hits);
        self.state_cache_code_misses.increment(stats.code_misses);
        self.state_cache_block_hash_hits.increment(stats.block_hash_hits);
        self.state_cache_block_hash_misses.increment(stats.block_hash_misses);
    }
}
