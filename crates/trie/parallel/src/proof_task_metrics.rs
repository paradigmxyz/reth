use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the proof task.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.proof_task")]
pub struct ProofTaskTrieMetrics {
    /// A histogram for the number of blinded account nodes fetched.
    blinded_account_nodes: Histogram,
    /// A histogram for the number of blinded storage nodes fetched.
    blinded_storage_nodes: Histogram,
    /// A histogram tracking time from starting storage worker spawn to when each worker marks
    /// itself available (after initialization, before grabbing first job).
    storage_worker_spawn_to_available_duration: Histogram,
    /// A histogram tracking time from starting account worker spawn to when each worker marks
    /// itself available (after initialization, before grabbing first job).
    account_worker_spawn_to_available_duration: Histogram,
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

    /// Record storage worker spawn-to-available duration.
    pub fn record_storage_worker_spawn_to_available_duration(&self, duration: std::time::Duration) {
        self.storage_worker_spawn_to_available_duration.record(duration.as_secs_f64());
    }

    /// Record account worker spawn-to-available duration.
    pub fn record_account_worker_spawn_to_available_duration(&self, duration: std::time::Duration) {
        self.account_worker_spawn_to_available_duration.record(duration.as_secs_f64());
    }
}
