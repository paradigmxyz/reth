use metrics::Histogram;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use std::time::{Duration, Instant};

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
    current_metrics: MakeCanonicalMetrics,
}

impl Default for MakeCanonicalDurationsRecorder {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            actions: Vec::new(),
            latest: None,
            current_metrics: MakeCanonicalMetrics::default(),
        }
    }
}

impl MakeCanonicalDurationsRecorder {
    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: MakeCanonicalAction) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        self.current_metrics.record(action, duration);
        self.latest = Some(elapsed);
    }
}

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
    /// Clearing trie updates of other children chains after fork choice update.
    ClearTrieUpdatesForOtherChildren,
}

/// Canonicalization metrics
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.make_canonical")]
struct MakeCanonicalMetrics {
    /// Duration of the clone old blocks action.
    clone_old_blocks: Histogram,
    /// Duration of the find canonical header action.
    find_canonical_header: Histogram,
    /// Duration of the split chain action.
    split_chain: Histogram,
    /// Duration of the split chain forks action.
    split_chain_forks: Histogram,
    /// Duration of the merge all chains action.
    merge_all_chains: Histogram,
    /// Duration of the update canonical index action.
    update_canonical_index: Histogram,
    /// Duration of the retrieve state trie updates action.
    retrieve_state_trie_updates: Histogram,
    /// Duration of the commit canonical chain to database action.
    commit_canonical_chain_to_database: Histogram,
    /// Duration of the revert canonical chain from database action.
    revert_canonical_chain_from_database: Histogram,
    /// Duration of the insert old canonical chain action.
    insert_old_canonical_chain: Histogram,
    /// Duration of the clear trie updates of other children chains after fork choice update
    /// action.
    clear_trie_updates_for_other_children: Histogram,
}

impl MakeCanonicalMetrics {
    /// Records the duration for the given action.
    pub(crate) fn record(&self, action: MakeCanonicalAction, duration: Duration) {
        match action {
            MakeCanonicalAction::CloneOldBlocks => self.clone_old_blocks.record(duration),
            MakeCanonicalAction::FindCanonicalHeader => self.find_canonical_header.record(duration),
            MakeCanonicalAction::SplitChain => self.split_chain.record(duration),
            MakeCanonicalAction::SplitChainForks => self.split_chain_forks.record(duration),
            MakeCanonicalAction::MergeAllChains => self.merge_all_chains.record(duration),
            MakeCanonicalAction::UpdateCanonicalIndex => {
                self.update_canonical_index.record(duration)
            }
            MakeCanonicalAction::RetrieveStateTrieUpdates => {
                self.retrieve_state_trie_updates.record(duration)
            }
            MakeCanonicalAction::CommitCanonicalChainToDatabase => {
                self.commit_canonical_chain_to_database.record(duration)
            }
            MakeCanonicalAction::RevertCanonicalChainFromDatabase => {
                self.revert_canonical_chain_from_database.record(duration)
            }
            MakeCanonicalAction::InsertOldCanonicalChain => {
                self.insert_old_canonical_chain.record(duration)
            }
            MakeCanonicalAction::ClearTrieUpdatesForOtherChildren => {
                self.clear_trie_updates_for_other_children.record(duration)
            }
        }
    }
}
