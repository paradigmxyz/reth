use reth_metrics::{metrics::Histogram, Metrics};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Metrics for blinded node fetching by proof workers.
#[derive(Clone, Debug)]
pub struct ProofTaskMetrics {
    /// The actual metrics for blinded nodes.
    pub task_metrics: ProofTaskTrieMetrics,
    /// Count of storage proof requests (lock-free).
    pub storage_proofs: Arc<AtomicU64>,
    /// Count of account proof requests (lock-free).
    pub account_proofs: Arc<AtomicU64>,
    /// Count of blinded account node requests (lock-free).
    pub account_nodes: Arc<AtomicU64>,
    /// Count of blinded storage node requests (lock-free).
    pub storage_nodes: Arc<AtomicU64>,
}

impl Default for ProofTaskMetrics {
    fn default() -> Self {
        Self {
            task_metrics: ProofTaskTrieMetrics::default(),
            storage_proofs: Arc::new(AtomicU64::new(0)),
            account_proofs: Arc::new(AtomicU64::new(0)),
            account_nodes: Arc::new(AtomicU64::new(0)),
            storage_nodes: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ProofTaskMetrics {
    /// Record the blinded node counts into the histograms.
    pub fn record(&self) {
        self.task_metrics.record_account_nodes(self.account_nodes.load(Ordering::Relaxed) as usize);
        self.task_metrics.record_storage_nodes(self.storage_nodes.load(Ordering::Relaxed) as usize);
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
