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

#[derive(Debug, Copy, Clone)]
pub(crate) enum MakeCanonicalAction {
    CloneOldBlocks,
    FindCanonicalHeader,
    SplitChain,
    SplitChainForks,
    MergeAllChains,
    UpdateCanonicalIndex,
    CommitCanonicalChainToDatabase,
    RevertCanonicalChainFromDatabase,
    InsertOldCanonicalChain,
}

impl MakeCanonicalAction {
    fn as_str(&self) -> &'static str {
        match self {
            MakeCanonicalAction::CloneOldBlocks => "clone old blocks",
            MakeCanonicalAction::FindCanonicalHeader => "find canonical header",
            MakeCanonicalAction::SplitChain => "split chain",
            MakeCanonicalAction::SplitChainForks => "split chain forks",
            MakeCanonicalAction::MergeAllChains => "merge all chains",
            MakeCanonicalAction::UpdateCanonicalIndex => "update canonical index",
            MakeCanonicalAction::CommitCanonicalChainToDatabase => {
                "commit canonical chain to database"
            }
            MakeCanonicalAction::RevertCanonicalChainFromDatabase => {
                "revert canonical chain from database"
            }
            MakeCanonicalAction::InsertOldCanonicalChain => "insert old canonical chain",
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
