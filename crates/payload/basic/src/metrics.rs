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
}
