use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for blinded node fetching for the duration of the proof task manager.
#[derive(Clone, Debug, Default)]
pub struct ProofTaskMetrics {
    /// The actual metrics for blinded nodes.
    pub task_metrics: ProofTaskTrieMetrics,
    /// Count of blinded account node requests.
    pub account_nodes: usize,
    /// Count of blinded storage node requests.
    pub storage_nodes: usize,
    /// Count of storage proof requests routed to worker pool.
    pub storage_proofs: usize,
    /// Count of times worker pool was full and fell back to on-demand execution.
    pub on_demand_fallback: usize,
}

impl ProofTaskMetrics {
    /// Record the blinded node counts into the histograms.
    pub fn record(&self) {
        self.task_metrics.record_account_nodes(self.account_nodes);
        self.task_metrics.record_storage_nodes(self.storage_nodes);
        self.task_metrics.record_storage_proofs(self.storage_proofs);
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
    /// A histogram for the number of storage proofs computed via worker pool.
    storage_proofs: Histogram,
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

    /// Record storage proofs computed via worker pool.
    pub fn record_storage_proofs(&self, count: usize) {
        self.storage_proofs.record(count as f64);
    }
}
