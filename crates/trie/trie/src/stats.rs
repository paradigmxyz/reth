use std::time::{Duration, Instant};

/// Trie stats.
#[derive(Clone, Copy, Debug)]
pub struct TrieStats {
    duration: Duration,
    branches_added: u64,
    leaves_added: u64,
}

impl TrieStats {
    /// Duration for root calculation.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Number of leaves added to the hash builder during the calculation.
    pub fn leaves_added(&self) -> u64 {
        self.leaves_added
    }

    /// Number of branches added to the hash builder during the calculation.
    pub fn branches_added(&self) -> u64 {
        self.branches_added
    }
}

/// Trie metrics tracker.
#[derive(Debug)]
pub struct TrieTracker {
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
    /// Increment the number of branches added to the hash builder during the calculation.
    pub fn inc_branch(&mut self) {
        self.branches_added += 1;
    }

    /// Increment the number of leaves added to the hash builder during the calculation.
    pub fn inc_leaf(&mut self) {
        self.leaves_added += 1;
    }

    /// Called when root calculation is finished to return trie statistics.
    pub fn finish(self) -> TrieStats {
        TrieStats {
            duration: self.started_at.elapsed(),
            branches_added: self.branches_added,
            leaves_added: self.leaves_added,
        }
    }
}
