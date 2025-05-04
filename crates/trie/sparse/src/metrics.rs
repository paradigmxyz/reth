//! Metrics for the sparse state trie

use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the sparse state trie
#[derive(Default, Debug)]
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
    pub(crate) histograms: SparseStateTrieHistograms,
}

impl SparseStateTrieMetrics {
    /// Record the metrics into the histograms
    pub(crate) fn record(&self) {
        self.histograms
            .multiproof_skipped_account_nodes
            .record(self.multiproof_skipped_account_nodes as f64);
        self.histograms
            .multiproof_total_account_nodes
            .record(self.multiproof_total_account_nodes as f64);
        self.histograms
            .multiproof_skipped_storage_nodes
            .record(self.multiproof_skipped_storage_nodes as f64);
        self.histograms
            .multiproof_total_storage_nodes
            .record(self.multiproof_total_storage_nodes as f64);
    }

    /// Increment the skipped account nodes counter
    pub(crate) fn increment_skipped_account_nodes(&mut self) {
        self.multiproof_skipped_account_nodes += 1;
    }

    /// Increment the total account nodes counter
    pub(crate) fn increment_total_account_nodes(&mut self) {
        self.multiproof_total_account_nodes += 1;
    }

    /// Increment the skipped storage nodes counter
    pub(crate) fn increment_skipped_storage_nodes(&mut self) {
        self.multiproof_skipped_storage_nodes += 1;
    }

    /// Increment the total storage nodes counter
    pub(crate) fn increment_total_storage_nodes(&mut self) {
        self.multiproof_total_storage_nodes += 1;
    }
}

/// Metrics for the sparse state trie
#[derive(Metrics)]
#[metrics(scope = "sparse_state_trie")]
pub(crate) struct SparseStateTrieHistograms {
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
