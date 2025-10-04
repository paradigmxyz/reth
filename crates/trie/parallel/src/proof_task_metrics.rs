use reth_metrics::{
    metrics::{Gauge, Histogram},
    Metrics,
};
use std::time::Duration;

/// Metrics for blinded node fetching for the duration of the proof task manager.
#[derive(Clone, Debug, Default)]
pub struct ProofTaskMetrics {
    /// The actual metrics for blinded nodes.
    pub task_metrics: ProofTaskTrieMetrics,
    /// Count of blinded account node requests.
    pub account_nodes: usize,
    /// Count of blinded storage node requests.
    pub storage_nodes: usize,
}

impl ProofTaskMetrics {
    /// Record the blinded node counts into the histograms.
    pub fn record(&self) {
        self.task_metrics.record_account_nodes(self.account_nodes);
        self.task_metrics.record_storage_nodes(self.storage_nodes);
    }

    /// Record the current depth of the storage queue.
    pub fn record_storage_queue_depth(&self, depth: usize) {
        self.task_metrics.record_storage_queue_depth(depth);
    }

    /// Record the current depth of the account queue.
    pub fn record_account_queue_depth(&self, depth: usize) {
        self.task_metrics.record_account_queue_depth(depth);
    }

    /// Record how long a storage job waited in the queue.
    pub fn record_storage_wait_time(&self, wait: Duration) {
        self.task_metrics.record_storage_wait_time(wait);
    }

    /// Record how long an account job waited in the queue.
    pub fn record_account_wait_time(&self, wait: Duration) {
        self.task_metrics.record_account_wait_time(wait);
    }
}

/// Metrics for the proof task.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.proof_task")]
pub struct ProofTaskTrieMetrics {
    /// A histogram for the number of blinded account nodes fetched.
    blinded_account_nodes: Histogram,
    /// A histogram for the number of blinded storage nodes fetched.
    blinded_storage_nodes: Histogram,
    /// Gauge tracking storage queue depth.
    storage_queue_depth: Gauge,
    /// Gauge tracking account queue depth.
    account_queue_depth: Gauge,
    /// Histogram capturing how long storage jobs wait in the queue (seconds).
    storage_wait_time_seconds: Histogram,
    /// Histogram capturing how long account jobs wait in the queue (seconds).
    account_wait_time_seconds: Histogram,
}

impl ProofTaskTrieMetrics {
    /// Record account nodes fetched.
    pub fn record_account_nodes(&self, count: usize) {
        self.blinded_account_nodes.record(count as f64);
    }

    /// Record storage nodes fetched.
    pub fn record_storage_nodes(&self, count: usize) {
        self.blinded_storage_nodes.record(count as f64);
    }

    /// Update the storage queue depth gauge.
    pub fn record_storage_queue_depth(&self, depth: usize) {
        self.storage_queue_depth.set(depth as f64);
    }

    /// Update the account queue depth gauge.
    pub fn record_account_queue_depth(&self, depth: usize) {
        self.account_queue_depth.set(depth as f64);
    }

    /// Record the time a storage job spent waiting in the queue.
    pub fn record_storage_wait_time(&self, wait: Duration) {
        self.storage_wait_time_seconds.record(wait.as_secs_f64());
    }

    /// Record the time an account job spent waiting in the queue.
    pub fn record_account_wait_time(&self, wait: Duration) {
        self.account_wait_time_seconds.record(wait.as_secs_f64());
    }
}
