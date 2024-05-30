use metrics::Histogram;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use std::time::{Duration, Instant};

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub struct TreeMetrics {
    /// Total number of sidechains (not including the canonical chain)
    pub sidechains: Gauge,
    /// The highest block number in the canonical chain
    pub canonical_chain_height: Gauge,
    /// The number of reorgs
    pub reorgs: Counter,
    /// The latest reorg depth
    pub latest_reorg_depth: Gauge,
    /// Longest sidechain height
    pub longest_sidechain_height: Gauge,
    /// The number of times cached trie updates were used for insert.
    pub trie_updates_insert_cached: Counter,
    /// The number of times trie updates were recomputed for insert.
    pub trie_updates_insert_recomputed: Counter,
}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub struct BlockBufferMetrics {
    /// Total blocks in the block buffer
    pub blocks: Gauge,
}

#[derive(Debug)]
pub(crate) struct MakeCanonicalDurationsRecorder {
    start: Instant,
    pub(crate) actions: Vec<(MakeCanonicalAction, Duration)>,
    latest: Option<Duration>,
}

impl Default for MakeCanonicalDurationsRecorder {
    fn default() -> Self {
        Self { start: Instant::now(), actions: Vec::new(), latest: None }
    }
}

impl MakeCanonicalDurationsRecorder {
    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: MakeCanonicalAction) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        MakeCanonicalMetrics::new_with_labels(&[("action", action.as_str())])
            .duration
            .record(duration);

        self.latest = Some(elapsed);
    }
}

/// Represents actions for making a canonical chain.
#[derive(Debug, Copy, Clone)]
pub(crate) enum MakeCanonicalAction {
    /// Cloning old blocks for canonicalization.
    CloneOldBlocks,
    /// Finding the canonical header.
    FindCanonicalHeader,
    /// Splitting the chain for canonicalization.
    SplitChain,
    /// Splitting chain forks for canonicalization.
    SplitChainForks,
    /// Merging all chains for canonicalization.
    MergeAllChains,
    /// Updating the canonical index during canonicalization.
    UpdateCanonicalIndex,
    /// Retrieving (cached or recomputed) state trie updates
    RetrieveStateTrieUpdates,
    /// Committing the canonical chain to the database.
    CommitCanonicalChainToDatabase,
    /// Reverting the canonical chain from the database.
    RevertCanonicalChainFromDatabase,
    /// Inserting an old canonical chain.
    InsertOldCanonicalChain,
    /// Clearing trie updates of other childs chains after fork choice update.
    ClearTrieUpdatesForOtherChilds,
}

impl MakeCanonicalAction {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::CloneOldBlocks => "clone old blocks",
            Self::FindCanonicalHeader => "find canonical header",
            Self::SplitChain => "split chain",
            Self::SplitChainForks => "split chain forks",
            Self::MergeAllChains => "merge all chains",
            Self::UpdateCanonicalIndex => "update canonical index",
            Self::RetrieveStateTrieUpdates => "retrieve state trie updates",
            Self::CommitCanonicalChainToDatabase => "commit canonical chain to database",
            Self::RevertCanonicalChainFromDatabase => "revert canonical chain from database",
            Self::InsertOldCanonicalChain => "insert old canonical chain",
            Self::ClearTrieUpdatesForOtherChilds => {
                "clear trie updates of other childs chains after fork choice update"
            }
        }
    }
}

#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.make_canonical")]
/// Canonicalization metrics
struct MakeCanonicalMetrics {
    /// The time it took to execute an action
    duration: Histogram,
}
