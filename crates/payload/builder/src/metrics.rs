//! Payloadbuild service metrics.

use metrics::{Counter, Gauge};
use reth_metrics_derive::Metrics;

/// Transaction pool metrics
#[derive(Metrics)]
#[metrics(scope = "payloads")]
pub(crate) struct PayloadBuilderServiceMetrics {
    /// Number of active jobs
    pub(crate) active_jobs: Gauge,
    /// Total number of initiated jobs
    pub(crate) initiated_jobs: Counter,
    /// Total number of failed jobs
    pub(crate) failed_jobs: Counter,
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
}
