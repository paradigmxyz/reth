use reth_metrics::{metrics::Histogram, Metrics};
use reth_trie::{
    hashed_cursor::metrics::{HashedCursorMetrics, HashedCursorMetricsCache},
    trie_cursor::metrics::{TrieCursorMetrics, TrieCursorMetricsCache},
    TrieType,
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

/// Cached cursor metrics for proof task operations.
#[derive(Clone, Debug, Default, Copy)]
pub struct ProofTaskCursorMetricsCache {
    /// Cached metrics for account trie cursor operations.
    pub account_trie_cursor: TrieCursorMetricsCache,
    /// Cached metrics for account hashed cursor operations.
    pub account_hashed_cursor: HashedCursorMetricsCache,
    /// Cached metrics for storage trie cursor operations.
    pub storage_trie_cursor: TrieCursorMetricsCache,
    /// Cached metrics for storage hashed cursor operations.
    pub storage_hashed_cursor: HashedCursorMetricsCache,
}

impl ProofTaskCursorMetricsCache {
    /// Extend this cache by adding the counts from another cache.
    ///
    /// This accumulates the counter values from `other` into this cache.
    pub const fn extend(&mut self, other: &Self) {
        self.account_trie_cursor.extend(&other.account_trie_cursor);
        self.account_hashed_cursor.extend(&other.account_hashed_cursor);
        self.storage_trie_cursor.extend(&other.storage_trie_cursor);
        self.storage_hashed_cursor.extend(&other.storage_hashed_cursor);
    }

    /// Record the cached metrics to the provided Prometheus metrics and reset counters.
    ///
    /// This method adds the current counter values to the Prometheus metrics
    /// and then resets all internal counters to zero.
    pub fn record(&mut self, metrics: &mut ProofTaskCursorMetrics) {
        self.account_trie_cursor.record(&mut metrics.account_trie_cursor);
        self.account_hashed_cursor.record(&mut metrics.account_hashed_cursor);
        self.storage_trie_cursor.record(&mut metrics.storage_trie_cursor);
        self.storage_hashed_cursor.record(&mut metrics.storage_hashed_cursor);
        self.reset();
    }

    /// Reset all counters to zero.
    pub const fn reset(&mut self) {
        self.account_trie_cursor.reset();
        self.account_hashed_cursor.reset();
        self.storage_trie_cursor.reset();
        self.storage_hashed_cursor.reset();
    }
}
