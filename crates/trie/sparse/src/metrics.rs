//! Metrics for the sparse state trie

use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the sparse state trie
#[derive(Debug, Default)]
pub(crate) struct SparseStateTrieMetrics {
    /// Number of account nodes that were skipped during a multiproof reveal due to being redundant
    /// (i.e. they were already revealed)
    pub(crate) multiproof_skipped_account_nodes: u64,
    /// Number of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: u64,
    /// Number of storage nodes that were skipped during a multiproof reveal due to being redundant
    /// (i.e. they were already revealed)
    pub(crate) multiproof_skipped_storage_nodes: u64,
    /// Number of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: u64,
    /// The actual metrics we will record into the histogram
    pub(crate) histograms: SparseStateTrieInnerMetrics,
    /// Metrics for trie pruning
    pub(crate) prune: PruneTrieMetrics,
}

impl SparseStateTrieMetrics {
    /// Record the metrics into the histograms
    pub(crate) fn record(&mut self) {
        use core::mem::take;
        self.histograms
            .multiproof_skipped_account_nodes
            .record(take(&mut self.multiproof_skipped_account_nodes) as f64);
        self.histograms
            .multiproof_total_account_nodes
            .record(take(&mut self.multiproof_total_account_nodes) as f64);
        self.histograms
            .multiproof_skipped_storage_nodes
            .record(take(&mut self.multiproof_skipped_storage_nodes) as f64);
        self.histograms
            .multiproof_total_storage_nodes
            .record(take(&mut self.multiproof_total_storage_nodes) as f64);
    }

    /// Increment the skipped account nodes counter by the given count
    pub(crate) const fn increment_skipped_account_nodes(&mut self, count: u64) {
        self.multiproof_skipped_account_nodes += count;
    }

    /// Increment the total account nodes counter by the given count
    pub(crate) const fn increment_total_account_nodes(&mut self, count: u64) {
        self.multiproof_total_account_nodes += count;
    }

    /// Increment the skipped storage nodes counter by the given count
    pub(crate) const fn increment_skipped_storage_nodes(&mut self, count: u64) {
        self.multiproof_skipped_storage_nodes += count;
    }

    /// Increment the total storage nodes counter by the given count
    pub(crate) const fn increment_total_storage_nodes(&mut self, count: u64) {
        self.multiproof_total_storage_nodes += count;
    }
}

/// Metrics for the sparse state trie
#[derive(Metrics)]
#[metrics(scope = "sparse_state_trie")]
pub(crate) struct SparseStateTrieInnerMetrics {
    /// Histogram of account nodes that were skipped during a multiproof reveal due to being
    /// redundant (i.e. they were already revealed)
    pub(crate) multiproof_skipped_account_nodes: Histogram,
    /// Histogram of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: Histogram,
    /// Histogram of storage nodes that were skipped during a multiproof reveal due to being
    /// redundant (i.e. they were already revealed)
    pub(crate) multiproof_skipped_storage_nodes: Histogram,
    /// Histogram of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: Histogram,
}

/// Metrics for trie pruning statistics
#[derive(Metrics, Clone)]
#[metrics(scope = "sparse_trie_prune")]
pub struct PruneTrieMetrics {
    /// Histogram of subtries skipped during pruning due to recent modifications
    pub skipped_modified: Histogram,
    /// Histogram of paths skipped during pruning because they lead to hot accounts
    pub skipped_hot_accounts: Histogram,
    /// Histogram of storage tries preserved due to hot account status
    pub hot_storage_tries_preserved: Histogram,
    /// Histogram of storage tries evicted (cold accounts)
    pub cold_storage_tries_evicted: Histogram,
    /// Histogram of storage tries that were depth-pruned
    pub storage_tries_pruned: Histogram,
    /// Histogram of storage tries depth-pruning duration in microseconds
    pub storage_tries_prune_duration: Histogram,
    /// Histogram of state trie pruning duration in microseconds
    pub state_prune_duration: Histogram,
    /// Histogram of storage tries pruning duration in microseconds
    pub storage_prune_duration: Histogram,
    /// Histogram of state trie memory usage in bytes after pruning
    pub state_memory_bytes: Histogram,
    /// Histogram of total storage tries memory usage in bytes after pruning
    pub storage_memory_bytes: Histogram,

    // Granular timing metrics for StorageTries::prune_preserving sections
    /// Time spent updating access tracking and resetting per-cycle flags (microseconds)
    pub storage_update_tracking_duration: Histogram,
    /// Time spent categorizing tries by hotness and collecting memory sizes (microseconds)
    pub storage_categorize_duration: Histogram,
    /// Time spent sorting cold tries by heat (microseconds)
    pub storage_sort_duration: Histogram,
    /// Time spent evicting cold tries from HashMap (microseconds)
    pub storage_eviction_duration: Histogram,
    /// Time spent calculating final memory after pruning (microseconds)
    pub storage_memory_calc_duration: Histogram,
    /// Number of tries iterated during categorization
    pub storage_tries_categorized: Histogram,
}
