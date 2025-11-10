use reth_metrics::{metrics::Histogram, Metrics};
use reth_trie::{
    hashed_cursor::metrics::HashedCursorMetrics, trie_cursor::metrics::TrieCursorMetrics, TrieType,
};

/// Metrics for the proof task.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.proof_task")]
pub struct ProofTaskTrieMetrics {
    /// A histogram for the number of blinded account nodes fetched.
    blinded_account_nodes: Histogram,
    /// A histogram for the number of blinded storage nodes fetched.
    blinded_storage_nodes: Histogram,
}

/// Cursor metrics for proof task operations.
#[derive(Clone, Debug)]
pub struct ProofTaskCursorMetrics {
    /// Metrics for account trie cursor operations.
    pub account_trie_cursor: TrieCursorMetrics,
    /// Metrics for account hashed cursor operations.
    pub account_hashed_cursor: HashedCursorMetrics,
    /// Metrics for storage trie cursor operations.
    pub storage_trie_cursor: TrieCursorMetrics,
    /// Metrics for storage hashed cursor operations.
    pub storage_hashed_cursor: HashedCursorMetrics,
}

impl ProofTaskCursorMetrics {
    /// Create a new instance with properly initialized cursor metrics.
    pub fn new() -> Self {
        Self {
            account_trie_cursor: TrieCursorMetrics::new(TrieType::State),
            account_hashed_cursor: HashedCursorMetrics::new(TrieType::State),
            storage_trie_cursor: TrieCursorMetrics::new(TrieType::Storage),
            storage_hashed_cursor: HashedCursorMetrics::new(TrieType::Storage),
        }
    }
}

impl Default for ProofTaskCursorMetrics {
    fn default() -> Self {
        Self::new()
    }
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
