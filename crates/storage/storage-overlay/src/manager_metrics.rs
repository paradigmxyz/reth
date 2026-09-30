use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use std::time::Duration;

/// Metrics for state trie overlay management.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.block_validation.state_trie_overlay")]
pub(crate) struct StateTrieOverlayMetrics {
    /// Duration of overlay computation in seconds.
    pub(crate) overlay_computation_duration_seconds: Histogram,
    /// Number of requests satisfied by an existing overlay cache entry.
    pub(crate) overlay_cache_reuses: Counter,
    /// Number of overlay cache entries populated by computing an overlay.
    pub(crate) overlay_cache_fills: Counter,
    /// Time spent waiting for an in-progress overlay computation, in seconds.
    pub(crate) overlay_wait_duration_seconds: Histogram,
}

/// Metrics for execution overlay management.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.block_validation.execution_overlay")]
pub(crate) struct ExecutionOverlayMetrics {
    /// Duration of overlay computation in seconds.
    pub(crate) overlay_computation_duration_seconds: Histogram,
    /// Number of requests satisfied by an existing overlay cache entry.
    pub(crate) overlay_cache_reuses: Counter,
    /// Number of overlay cache entries populated by computing an overlay.
    pub(crate) overlay_cache_fills: Counter,
    /// Time spent waiting for an in-progress overlay computation, in seconds.
    pub(crate) overlay_wait_duration_seconds: Histogram,
}

pub(crate) trait OverlayCacheMetrics {
    fn record_cache_reuse(&self);

    fn record_cache_fill(&self);

    fn record_wait_duration(&self, duration: Duration);
}

impl OverlayCacheMetrics for StateTrieOverlayMetrics {
    fn record_cache_reuse(&self) {
        self.overlay_cache_reuses.increment(1);
    }

    fn record_cache_fill(&self) {
        self.overlay_cache_fills.increment(1);
    }

    fn record_wait_duration(&self, duration: Duration) {
        self.overlay_wait_duration_seconds.record(duration.as_secs_f64());
    }
}

impl OverlayCacheMetrics for ExecutionOverlayMetrics {
    fn record_cache_reuse(&self) {
        self.overlay_cache_reuses.increment(1);
    }

    fn record_cache_fill(&self) {
        self.overlay_cache_fills.increment(1);
    }

    fn record_wait_duration(&self, duration: Duration) {
        self.overlay_wait_duration_seconds.record(duration.as_secs_f64());
    }
}
