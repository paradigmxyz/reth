//! Metrics for the payload builder impl

use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};

/// Payload builder metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "payloads")]
pub(crate) struct PayloadBuilderMetrics {
    /// Total number of times an empty payload was returned because a built one was not ready.
    pub(crate) requested_empty_payload: Counter,
    /// Total number of initiated payload build attempts.
    pub(crate) initiated_payload_builds: Counter,
    /// Total number of failed payload build attempts.
    pub(crate) failed_payload_builds: Counter,
    /// Total time it took to build the payload in seconds.
    pub(crate) payload_build_duration_seconds: Histogram,
    /// Number of transactions included in the built payload.
    pub(crate) built_payload_transactions: Histogram,
    /// Gas used by the built payload.
    pub(crate) built_payload_gas_used: Gauge,
    /// RLP-encoded size of the built payload in bytes.
    pub(crate) built_payload_rlp_size: Histogram,
    /// Gas throughput measured as `gas_used` / `build_duration`.
    pub(crate) built_payload_gas_per_second: Gauge,
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
