use crate::{stats::TrieStats, trie::TrieType};
use metrics::{Counter, Histogram};
use reth_metrics::Metrics;

/// Wrapper for state root metrics.
#[derive(Debug)]
pub struct StateRootMetrics {
    /// State trie metrics.
    pub state_trie: TrieRootMetrics,
    /// Storage trie metrics.
    pub storage_trie: TrieRootMetrics,
}

impl Default for StateRootMetrics {
    fn default() -> Self {
        Self {
            state_trie: TrieRootMetrics::new(TrieType::State),
            storage_trie: TrieRootMetrics::new(TrieType::Storage),
        }
    }
}

/// Metrics for trie root calculation.
#[derive(Clone, Metrics)]
#[metrics(scope = "trie")]
pub struct TrieRootMetrics {
    /// The number of seconds trie root calculation lasted.
    duration_seconds: Histogram,
    /// The number of branches added during trie root calculation.
    branches_added: Histogram,
    /// The number of leaves added during trie root calculation.
    leaves_added: Histogram,
}

impl TrieRootMetrics {
    /// Create new metrics for the given trie type.
    pub fn new(ty: TrieType) -> Self {
        Self::new_with_labels(&[("type", ty.as_str())])
    }

    /// Record trie stats as metrics.
    pub fn record(&self, stats: TrieStats) {
        self.duration_seconds.record(stats.duration().as_secs_f64());
        self.branches_added.record(stats.branches_added() as f64);
        self.leaves_added.record(stats.leaves_added() as f64);
    }
}

/// Metrics for [`crate::walker::TrieWalker`].
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.walker")]
pub struct WalkerMetrics {
    /// The number of branch nodes seeked by the walker.
    branch_nodes_seeked_total: Counter,
    /// The number of subnodes out of order due to wrong tree mask.
    out_of_order_subnode: Counter,
}

impl WalkerMetrics {
    /// Create new metrics for the given trie type.
    pub fn new(ty: TrieType) -> Self {
        Self::new_with_labels(&[("type", ty.as_str())])
    }

    /// Increment `branch_nodes_seeked_total`.
    pub fn inc_branch_nodes_seeked(&self) {
        self.branch_nodes_seeked_total.increment(1);
    }

    /// Increment `out_of_order_subnode`.
    pub fn inc_out_of_order_subnode(&self, amount: u64) {
        self.out_of_order_subnode.increment(amount);
    }
}

/// Metrics for [`crate::node_iter::TrieNodeIter`].
#[derive(Clone, Metrics)]
#[metrics(scope = "trie.node_iter")]
pub struct TrieNodeIterMetrics {
    /// The number of branch nodes returned by the iterator.
    branch_nodes_returned_total: Counter,
    /// The number of times the same hashed cursor key was seeked multiple times in a row by the
    /// iterator. It does not mean the database seek was actually done, as the trie node
    /// iterator caches the last hashed cursor seek.
    leaf_nodes_same_seeked_total: Counter,
    /// The number of leaf nodes seeked by the iterator.
    leaf_nodes_seeked_total: Counter,
    /// The number of leaf nodes advanced by the iterator.
    leaf_nodes_advanced_total: Counter,
    /// The number of leaf nodes returned by the iterator.
    leaf_nodes_returned_total: Counter,
}

impl TrieNodeIterMetrics {
    /// Create new metrics for the given trie type.
    pub fn new(ty: TrieType) -> Self {
        Self::new_with_labels(&[("type", ty.as_str())])
    }

    /// Increment `branch_nodes_returned_total`.
    pub fn inc_branch_nodes_returned(&self) {
        self.branch_nodes_returned_total.increment(1);
    }

    /// Increment `leaf_nodes_same_seeked_total`.
    pub fn inc_leaf_nodes_same_seeked(&self) {
        self.leaf_nodes_same_seeked_total.increment(1);
    }

    /// Increment `leaf_nodes_seeked_total`.
    pub fn inc_leaf_nodes_seeked(&self) {
        self.leaf_nodes_seeked_total.increment(1);
    }

    /// Increment `leaf_nodes_advanced_total`.
    pub fn inc_leaf_nodes_advanced(&self) {
        self.leaf_nodes_advanced_total.increment(1);
    }

    /// Increment `leaf_nodes_returned_total`.
    pub fn inc_leaf_nodes_returned(&self) {
        self.leaf_nodes_returned_total.increment(1);
    }
}
