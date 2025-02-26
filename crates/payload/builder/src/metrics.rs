//! Payload builder service metrics.

use reth_metrics::{
    metrics::{Counter, Gauge},
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
