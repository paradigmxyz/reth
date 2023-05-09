//! Metrics for the payload builder impl

use metrics::Counter;
use reth_metrics_derive::Metrics;

/// Transaction pool metrics
#[derive(Metrics)]
#[metrics(scope = "payloads")]
pub(crate) struct PayloadBuilderMetrics {
    /// Number of active jobs
    pub(crate) requested_empty_payload: Counter,
    /// Total number of initiated payload build attempts
    pub(crate) initiated_payload_builds: Counter,
    /// Total number of failed payload build attempts
    pub(crate) failed_payload_builds: Counter,
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
}
