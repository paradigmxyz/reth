//! Proof worker metrics.

use reth_metrics::{metrics::Histogram, Metrics};
use std::time::Duration;

/// Idle time metrics of the proof workers.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.proof_task")]
pub struct ProofWorkerIdleMetrics {
    /// Histogram for storage worker idle time in seconds (waiting for proof jobs).
    storage_worker_idle_time_seconds: Histogram,
    /// Histogram for account worker idle time in seconds (waiting for proof jobs).
    account_worker_idle_time_seconds: Histogram,
}

impl ProofWorkerIdleMetrics {
    /// Record storage worker idle time.
    pub fn record_storage_worker_idle_time(&self, duration: Duration) {
        self.storage_worker_idle_time_seconds.record(duration.as_secs_f64());
    }

    /// Record account worker idle time.
    pub fn record_account_worker_idle_time(&self, duration: Duration) {
        self.account_worker_idle_time_seconds.record(duration.as_secs_f64());
    }
}
