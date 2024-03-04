use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::Instant;

#[derive(Metrics)]
#[metrics(scope = "trie")]
pub(crate) struct TrieRootMetrics {
    /// The number of seconds trie root calculation lasted.
    duration: Histogram,
    /// The number of branches added during trie root calculation.
    branches_added: Histogram,
    /// The number of leaves added during trie root calculation.
    leaves_added: Histogram,
}

impl TrieRootMetrics {
    pub(crate) fn new(ty: TrieType) -> Self {
        Self::new_with_labels(&[("type", ty.as_str())])
    }

    pub(crate) fn record(&self, tracker: TrieTracker) {
        self.duration.record(tracker.started_at.elapsed().as_secs_f64());
        self.branches_added.record(tracker.branches_added as f64);
        self.leaves_added.record(tracker.leaves_added as f64);
    }
}

/// Trie type for differentiating between various trie calculations.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TrieType {
    State,
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct TrieTracker {
    started_at: Instant,
    branches_added: u64,
    leaves_added: u64,
}

impl Default for TrieTracker {
    fn default() -> Self {
        Self { started_at: Instant::now(), branches_added: 0, leaves_added: 0 }
    }
}

impl TrieTracker {
    pub(crate) fn leaves_added(&self) -> u64 {
        self.leaves_added
    }

    pub(crate) fn inc_branch(&mut self) {
        self.branches_added += 1;
    }

    pub(crate) fn inc_leaf(&mut self) {
        self.leaves_added += 1;
    }
}
