use crate::value_encoder::ValueEncoderStats;
use reth_metrics::{metrics::Histogram, Metrics};
use reth_trie::{
    hashed_cursor::{HashedCursorMetrics, HashedCursorMetricsCache},
    trie_cursor::{TrieCursorMetrics, TrieCursorMetricsCache},
    TrieType,
};
use std::time::Duration;

/// Metrics for the proof task.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.proof_task")]
pub struct ProofTaskTrieMetrics {
    /// A histogram for the number of blinded account nodes fetched.
    blinded_account_nodes: Histogram,
    /// A histogram for the number of blinded storage nodes fetched.
    blinded_storage_nodes: Histogram,
    /// Histogram for storage worker idle time in seconds (waiting for proof jobs).
    storage_worker_idle_time_seconds: Histogram,
    /// Histogram for account worker idle time in seconds (waiting for proof jobs + storage
    /// results).
    account_worker_idle_time_seconds: Histogram,
    /// Histogram for `Dispatched` deferred encoder variant count.
    deferred_encoder_dispatched: Histogram,
    /// Histogram for `FromCache` deferred encoder variant count.
    deferred_encoder_from_cache: Histogram,
    /// Histogram for `Sync` deferred encoder variant count.
    deferred_encoder_sync: Histogram,
    /// Histogram for dispatched storage proofs that fell back to sync due to missing root.
    deferred_encoder_dispatched_missing_root: Histogram,
    /// Histogram for `InlineSingle` deferred encoder variant count (single-target storage proofs
    /// computed inline in the account worker).
    deferred_encoder_inline_single: Histogram,
    /// Histogram for time account workers spent blocked waiting for storage proof results
    /// (seconds). This is the portion of account worker idle time attributable to storage
    /// worker latency rather than queue wait.
    account_worker_storage_wait_seconds: Histogram,
}

impl ProofTaskTrieMetrics {
    /// Record account nodes fetched.
    pub fn record_account_nodes(&self, count: usize) {
        self.blinded_account_nodes.record(count as f64);
    }

    /// Record storage nodes fetched.
    pub fn record_storage_nodes(&self, count: usize) {
        self.blinded_storage_nodes.record(count as f64);
    }

    /// Record storage worker idle time.
    pub fn record_storage_worker_idle_time(&self, duration: Duration) {
        self.storage_worker_idle_time_seconds.record(duration.as_secs_f64());
    }

    /// Record account worker idle time.
    pub fn record_account_worker_idle_time(&self, duration: Duration) {
        self.account_worker_idle_time_seconds.record(duration.as_secs_f64());
    }

    /// Record value encoder stats (deferred encoder variant counts and storage wait time).
    pub(crate) fn record_value_encoder_stats(&self, stats: &ValueEncoderStats) {
        self.deferred_encoder_dispatched.record(stats.dispatched_count as f64);
        self.deferred_encoder_from_cache.record(stats.from_cache_count as f64);
        self.deferred_encoder_sync.record(stats.sync_count as f64);
        self.deferred_encoder_dispatched_missing_root
            .record(stats.dispatched_missing_root_count as f64);
        self.deferred_encoder_inline_single.record(stats.inline_single_count as f64);
        self.account_worker_storage_wait_seconds.record(stats.storage_wait_time.as_secs_f64());
    }
}

/// Cursor metrics for proof task operations.
#[derive(Clone, Debug)]
pub struct ProofTaskCursorMetrics {
    /// Metrics for account trie cursor operations.
    pub account_trie_cursor: TrieCursorMetrics,
    /// Metrics for account hashed cursor operations.
    pub account_hashed_cursor: HashedCursorMetrics,
    /// Metrics for storage trie cursor operations.
    pub storage_trie_cursor: TrieCursorMetrics,
    /// Metrics for storage hashed cursor operations.
    pub storage_hashed_cursor: HashedCursorMetrics,
}

impl ProofTaskCursorMetrics {
    /// Create a new instance with properly initialized cursor metrics.
    pub fn new() -> Self {
        Self {
            account_trie_cursor: TrieCursorMetrics::new(TrieType::State),
            account_hashed_cursor: HashedCursorMetrics::new(TrieType::State),
            storage_trie_cursor: TrieCursorMetrics::new(TrieType::Storage),
            storage_hashed_cursor: HashedCursorMetrics::new(TrieType::Storage),
        }
    }

    /// Record the cached metrics from the provided cache and reset the cache counters.
    ///
    /// This method adds the current counter values from the cache to the Prometheus metrics
    /// and then resets all cache counters to zero.
    pub fn record(&mut self, cache: &mut ProofTaskCursorMetricsCache) {
        self.account_trie_cursor.record(&mut cache.account_trie_cursor);
        self.account_hashed_cursor.record(&mut cache.account_hashed_cursor);
        self.storage_trie_cursor.record(&mut cache.storage_trie_cursor);
        self.storage_hashed_cursor.record(&mut cache.storage_hashed_cursor);
        cache.reset();
    }
}

impl Default for ProofTaskCursorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Cached cursor metrics for proof task operations.
#[derive(Clone, Debug, Default, Copy)]
pub struct ProofTaskCursorMetricsCache {
    /// Cached metrics for account trie cursor operations.
    pub account_trie_cursor: TrieCursorMetricsCache,
    /// Cached metrics for account hashed cursor operations.
    pub account_hashed_cursor: HashedCursorMetricsCache,
    /// Cached metrics for storage trie cursor operations.
    pub storage_trie_cursor: TrieCursorMetricsCache,
    /// Cached metrics for storage hashed cursor operations.
    pub storage_hashed_cursor: HashedCursorMetricsCache,
}

impl ProofTaskCursorMetricsCache {
    /// Extend this cache by adding the counts from another cache.
    ///
    /// This accumulates the counter values from `other` into this cache.
    pub fn extend(&mut self, other: &Self) {
        self.account_trie_cursor.extend(&other.account_trie_cursor);
        self.account_hashed_cursor.extend(&other.account_hashed_cursor);
        self.storage_trie_cursor.extend(&other.storage_trie_cursor);
        self.storage_hashed_cursor.extend(&other.storage_hashed_cursor);
    }

    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.account_trie_cursor.reset();
        self.account_hashed_cursor.reset();
        self.storage_trie_cursor.reset();
        self.storage_hashed_cursor.reset();
    }

    /// Record the spans for metrics.
    pub fn record_spans(&self) {
        self.account_trie_cursor.record_span("account_trie_cursor");
        self.account_hashed_cursor.record_span("account_hashed_cursor");
        self.storage_trie_cursor.record_span("storage_trie_cursor");
        self.storage_hashed_cursor.record_span("storage_hashed_cursor");
    }
}
