//! Payload builder service metrics.

use std::time::Duration;

use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};

/// Payload builder service metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "payloads")]
pub(crate) struct PayloadBuilderServiceMetrics {
    /// Number of active jobs
    pub(crate) active_jobs: Gauge,
    /// Total number of initiated jobs
    pub(crate) initiated_jobs: Counter,
    /// Total number of failed jobs
    pub(crate) failed_jobs: Counter,
    /// Coinbase revenue for best payloads
    pub(crate) best_revenue: Gauge,
    /// Current block returned as the best payload
    pub(crate) best_block: Gauge,
    /// Coinbase revenue for resolved payloads
    pub(crate) resolved_revenue: Gauge,
    /// Current block returned as the resolved payload
    pub(crate) resolved_block: Gauge,
    /// Histogram of payload resolve latency in seconds
    pub(crate) resolve_duration_seconds: Histogram,
    /// Histogram of new payload job creation latency in seconds
    pub(crate) new_job_duration_seconds: Histogram,
}

impl PayloadBuilderServiceMetrics {
    pub(crate) fn inc_initiated_jobs(&self) {
        self.initiated_jobs.increment(1);
    }

    pub(crate) fn inc_failed_jobs(&self) {
        self.failed_jobs.increment(1);
    }

    pub(crate) fn set_active_jobs(&self, value: usize) {
        self.active_jobs.set(value as f64)
    }

    pub(crate) fn set_best_revenue(&self, block: u64, value: f64) {
        self.best_block.set(block as f64);
        self.best_revenue.set(value)
    }

    pub(crate) fn set_resolved_revenue(&self, block: u64, value: f64) {
        self.resolved_block.set(block as f64);
        self.resolved_revenue.set(value)
    }
}

/// State-root computation mode used by a payload build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadBuilderStateRootMode {
    /// Sparse-trie handle shared from the canonical forkchoice path.
    SharedSparseTrie,
    /// Sparse-trie handle created for an explicit speculative state anchor.
    SpeculativeSparseTrie,
    /// Synchronous `state_root_with_updates` path.
    SyncFallback,
}

impl PayloadBuilderStateRootMode {
    /// Returns the metric label for this mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SharedSparseTrie => "shared_sparse_trie",
            Self::SpeculativeSparseTrie => "speculative_sparse_trie",
            Self::SyncFallback => "sync_fallback",
        }
    }
}

/// Records the state-root mode selected for a payload build.
pub fn record_payload_builder_state_root_mode(mode: PayloadBuilderStateRootMode) {
    metrics::counter!("payloads_state_root_mode_total", "mode" => mode.as_str()).increment(1);
}

/// Records the time spent waiting for a sparse-trie handle to finish.
pub fn record_payload_builder_sparse_trie_wait(
    mode: PayloadBuilderStateRootMode,
    duration: Duration,
) {
    metrics::histogram!(
        "payloads_state_root_sparse_trie_wait_seconds",
        "mode" => mode.as_str()
    )
    .record(duration.as_secs_f64());
}

/// Records a synchronous state-root fallback used while building a payload.
pub fn record_payload_builder_sync_state_root(
    mode: PayloadBuilderStateRootMode,
    reason: &'static str,
    duration: Duration,
) {
    metrics::counter!(
        "payloads_state_root_sync_fallback_total",
        "mode" => mode.as_str(),
        "reason" => reason
    )
    .increment(1);
    metrics::histogram!(
        "payloads_state_root_sync_fallback_duration_seconds",
        "mode" => mode.as_str(),
        "reason" => reason
    )
    .record(duration.as_secs_f64());
}
