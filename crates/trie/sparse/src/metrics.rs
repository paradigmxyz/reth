//! Metrics for the sparse state trie

use metrics::Gauge;
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
    /// The actual metrics we will record
    pub(crate) inner_metrics: SparseStateTrieInnerMetrics,
}

impl SparseStateTrieMetrics {
    /// Record the metrics into the histograms
    pub(crate) fn record(&mut self) {
        use core::mem::take;
        self.inner_metrics
            .multiproof_skipped_account_nodes
            .record(take(&mut self.multiproof_skipped_account_nodes) as f64);
        self.inner_metrics
            .multiproof_total_account_nodes
            .record(take(&mut self.multiproof_total_account_nodes) as f64);
        self.inner_metrics
            .multiproof_skipped_storage_nodes
            .record(take(&mut self.multiproof_skipped_storage_nodes) as f64);
        self.inner_metrics
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

    /// Set the value capacity for the sparse state trie
    pub(crate) fn set_value_capacity(&self, capacity: usize) {
        self.inner_metrics.value_capacity.set(capacity as f64);
    }

    /// Set the node capacity for the sparse state trie
    pub(crate) fn set_node_capacity(&self, capacity: usize) {
        self.inner_metrics.node_capacity.set(capacity as f64);
    }

    /// Set the number of cleared and active storage tries
    pub(crate) fn set_storage_trie_metrics(&self, cleared: usize, active: usize) {
        self.inner_metrics.cleared_storage_tries.set(cleared as f64);
        self.inner_metrics.active_storage_tries.set(active as f64);
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
    /// Gauge for the trie's node capacity
    pub(crate) node_capacity: Gauge,
    /// Gauge for the trie's value capacity
    pub(crate) value_capacity: Gauge,
    /// The current number of cleared storage tries.
    pub(crate) cleared_storage_tries: Gauge,
    /// The number of currently active storage tries, i.e., not cleared
    pub(crate) active_storage_tries: Gauge,
}
