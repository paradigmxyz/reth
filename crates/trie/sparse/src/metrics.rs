//! Metrics for the sparse state trie

use reth_metrics::{metrics::Histogram, Metrics};

/// Metrics for the sparse state trie
#[derive(Default, Debug)]
pub(crate) struct SparseStateTrieMetrics {
    /// Number of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: u64,
    /// Number of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: u64,
    /// The actual metrics we will record into the histogram
    pub(crate) histograms: SparseStateTrieInnerMetrics,
}

impl SparseStateTrieMetrics {
    /// Record the metrics into the histograms
    pub(crate) fn record(&mut self) {
        use core::mem::take;
        self.histograms
            .multiproof_total_account_nodes
            .record(take(&mut self.multiproof_total_account_nodes) as f64);
        self.histograms
            .multiproof_total_storage_nodes
            .record(take(&mut self.multiproof_total_storage_nodes) as f64);
    }

    /// Increment the total account nodes counter by the given count
    pub(crate) const fn increment_total_account_nodes(&mut self, count: u64) {
        self.multiproof_total_account_nodes += count;
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
    /// Histogram of total account nodes, including those that were skipped.
    pub(crate) multiproof_total_account_nodes: Histogram,
    /// Histogram of total storage nodes, including those that were skipped.
    pub(crate) multiproof_total_storage_nodes: Histogram,
}

/// Metrics for the parallel sparse trie
#[derive(Metrics, Clone)]
#[metrics(scope = "parallel_sparse_trie")]
pub(crate) struct ParallelSparseTrieMetrics {
    /// A histogram for the number of subtries updated when calculating hashes.
    pub(crate) subtries_updated: Histogram,
    /// A histogram for the time it took to update lower subtrie hashes.
    pub(crate) subtrie_hash_update_latency: Histogram,
    /// A histogram for the time it took to update the upper subtrie hashes.
    pub(crate) subtrie_upper_hash_latency: Histogram,
}

impl PartialEq for ParallelSparseTrieMetrics {
    fn eq(&self, _other: &Self) -> bool {
        // It does not make sense to compare metrics, so return true, all are equal
        true
    }
}

impl Eq for ParallelSparseTrieMetrics {}

/// Metrics for measuring node churn by depth in the arena sparse trie.
///
/// "Churn" at a node means that node was on the cursor stack path for more than one
/// update in a single `update_leaves` call. This measures how often nodes at each
/// depth are touched multiple times, which informs whether partial RLP caching
/// during `update_leaves` would be viable below a certain depth.
#[derive(Default, Debug, Clone)]
pub(crate) struct ArenaChurnMetrics {
    histograms: ArenaChurnInnerMetrics,
}

impl ArenaChurnMetrics {
    /// Record churn data for a single `update_leaves` call.
    ///
    /// For each node depth, emits:
    /// - `nodes_with_churn`: how many nodes at that depth were touched by >1 update
    /// - `total_extra_touches`: sum of (touch_count - 1) for nodes with churn
    /// - `total_nodes_touched`: total nodes touched at that depth
    pub(crate) fn record(
        &self,
        depth: usize,
        nodes_with_churn: u64,
        total_extra_touches: u64,
        total_nodes_touched: u64,
        trie_type: &str,
    ) {
        let depth_str = alloc::format!("{}", depth);
        self.histograms.nodes_with_churn(trie_type, &depth_str).record(nodes_with_churn as f64);
        self.histograms
            .total_extra_touches(trie_type, &depth_str)
            .record(total_extra_touches as f64);
        self.histograms
            .total_nodes_touched(trie_type, &depth_str)
            .record(total_nodes_touched as f64);
    }
}

/// Inner metrics using the `metrics` crate directly (not the derive macro, since we
/// need dynamic labels for depth).
#[derive(Default, Debug, Clone)]
struct ArenaChurnInnerMetrics;

impl ArenaChurnInnerMetrics {
    fn nodes_with_churn(&self, trie_type: &str, depth: &str) -> Histogram {
        metrics::histogram!("arena_sparse_trie.churn.nodes_with_churn", "trie_type" => trie_type.to_string(), "depth" => depth.to_string())
    }

    fn total_extra_touches(&self, trie_type: &str, depth: &str) -> Histogram {
        metrics::histogram!("arena_sparse_trie.churn.total_extra_touches", "trie_type" => trie_type.to_string(), "depth" => depth.to_string())
    }

    fn total_nodes_touched(&self, trie_type: &str, depth: &str) -> Histogram {
        metrics::histogram!("arena_sparse_trie.churn.total_nodes_touched", "trie_type" => trie_type.to_string(), "depth" => depth.to_string())
    }
}

extern crate alloc;
