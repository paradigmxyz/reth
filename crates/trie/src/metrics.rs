use crate::stats::TrieStats;
use metrics::Histogram;
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

/// Trie type for differentiating between various trie calculations.
#[derive(Clone, Copy, Debug)]
pub enum TrieType {
    /// State trie type.
    State,
    /// Storage trie type.
    Storage,
}

impl TrieType {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Storage => "storage",
        }
    }
}
