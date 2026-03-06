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
    /// Time spent fetching the parent state provider.
    pub(crate) state_provider_duration_seconds: Histogram,
    /// Time spent constructing the state DB wrapper.
    pub(crate) build_state_db_duration_seconds: Histogram,
    /// Time spent creating the EVM/block builder.
    pub(crate) create_evm_duration_seconds: Histogram,
    /// Time spent applying pre-execution changes.
    pub(crate) pre_execution_duration_seconds: Histogram,
    /// Time spent selecting and executing transactions.
    pub(crate) execute_transactions_duration_seconds: Histogram,
    /// Time spent finalizing and sealing the block.
    pub(crate) finish_block_duration_seconds: Histogram,
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

    /// Records the time spent fetching the parent state provider.
    pub fn record_state_provider_duration(&self, duration_secs: f64) {
        self.state_provider_duration_seconds.record(duration_secs);
    }

    /// Records the time spent constructing the state DB wrapper.
    pub fn record_build_state_db_duration(&self, duration_secs: f64) {
        self.build_state_db_duration_seconds.record(duration_secs);
    }

    /// Records the time spent creating the EVM/block builder.
    pub fn record_create_evm_duration(&self, duration_secs: f64) {
        self.create_evm_duration_seconds.record(duration_secs);
    }

    /// Records the time spent applying pre-execution changes.
    pub fn record_pre_execution_duration(&self, duration_secs: f64) {
        self.pre_execution_duration_seconds.record(duration_secs);
    }

    /// Records the time spent selecting and executing transactions.
    pub fn record_execute_transactions_duration(&self, duration_secs: f64) {
        self.execute_transactions_duration_seconds.record(duration_secs);
    }

    /// Records the time spent finalizing and sealing the block.
    pub fn record_finish_block_duration(&self, duration_secs: f64) {
        self.finish_block_duration_seconds.record(duration_secs);
    }
}
