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

#[derive(Debug, Default)]
pub(crate) struct MakeCanonicalDurationsRecorder {
    pub(crate) actions: Vec<(MakeCanonicalAction, Duration)>,
}

impl MakeCanonicalDurationsRecorder {
    /// Records the duration of `f` execution, saves for future logging and instantly reports as a
    /// metric with `action` label.
    pub(crate) fn record<T>(&mut self, action: MakeCanonicalAction, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = f();
        let elapsed = start.elapsed();

        self.actions.push((action, elapsed));
        MakeCanonicalMetrics::new_with_labels(&[("action", format!("{action:?}"))])
            .duration
            .record(elapsed);

        result
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

#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.make_canonical")]
/// Canonicalization metrics
struct MakeCanonicalMetrics {
    /// The time it took to execute an action
    duration: Histogram,
}
