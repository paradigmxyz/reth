//! Payload builder service metrics.

use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::FastInstant as Instant;

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
    /// Age of the oldest in-flight payload job in seconds.
    /// Resets to 0 when no jobs are active. A sustained high value
    /// indicates a stuck build (e.g. deadlocked finalization or state root).
    pub(crate) oldest_active_job_age_seconds: Gauge,
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

    /// Updates the oldest active job age gauge from the current set of in-flight jobs.
    pub(crate) fn set_oldest_job_age<T>(
        &self,
        jobs: &[(T, alloy_rpc_types::engine::PayloadId, tracing::Span, Instant)],
    ) {
        let age =
            jobs.iter().map(|(_, _, _, created_at)| created_at.elapsed()).max().unwrap_or_default();
        self.oldest_active_job_age_seconds.set(age.as_secs_f64());
    }
}
