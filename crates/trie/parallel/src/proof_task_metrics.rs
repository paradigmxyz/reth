use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for blinded node fetching by proof workers.
#[derive(Clone, Debug, Default)]
pub struct ProofTaskMetrics {
    /// The actual metrics for blinded nodes.
    pub task_metrics: ProofTaskTrieMetrics,
}

impl ProofTaskMetrics {
    /// Record the blinded account node count into the histogram.
    pub fn record_account_nodes(&self, count: usize) {
        self.task_metrics.record_account_nodes(count);
    }

    /// Record the blinded storage node count into the histogram.
    pub fn record_storage_nodes(&self, count: usize) {
        self.task_metrics.record_storage_nodes(count);
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
}
