use reth_evm::metrics::ExecutorMetrics;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_trie::updates::TrieUpdates;

/// Metrics for the `EngineApi`.
#[derive(Debug, Default)]
pub(crate) struct EngineApiMetrics {
    /// Engine API-specific metrics.
    pub(crate) engine: EngineMetrics,
    /// Block executor metrics.
    pub(crate) executor: ExecutorMetrics,
    /// Metrics for block validation
    pub(crate) block_validation: BlockValidationMetrics,
    /// A copy of legacy blockchain tree metrics, to be replaced when we replace the old tree
    pub(crate) tree: TreeMetrics,
}

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub(super) struct TreeMetrics {
    /// The highest block number in the canonical chain
    pub canonical_chain_height: Gauge,
    /// The number of reorgs
    pub reorgs: Counter,
    /// The latest reorg depth
    pub latest_reorg_depth: Gauge,
}

/// Metrics for the `EngineApi`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineMetrics {
    /// How many executed blocks are currently stored.
    pub(crate) executed_blocks: Gauge,
    /// How many already executed blocks were directly inserted into the tree.
    pub(crate) inserted_already_executed_blocks: Counter,
    /// The number of times the pipeline was run.
    pub(crate) pipeline_runs: Counter,
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
    /// Histogram of persistence operation durations (in seconds)
    pub(crate) persistence_duration: Histogram,
    /// Tracks the how often we failed to deliver a newPayload response.
    ///
    /// This effectively tracks how often the message sender dropped the channel and indicates a CL
    /// request timeout (e.g. it took more than 8s to send the response and the CL terminated the
    /// request which resulted in a closed channel).
    pub(crate) failed_new_payload_response_deliveries: Counter,
    /// Tracks the how often we failed to deliver a forkchoice update response.
    pub(crate) failed_forkchoice_updated_response_deliveries: Counter,
    // TODO add latency metrics
}

/// Metrics for non-execution related block validation.
#[derive(Metrics)]
#[metrics(scope = "sync.block_validation")]
pub(crate) struct BlockValidationMetrics {
    /// Total number of storage tries updated in the state root calculation
    pub(crate) state_root_storage_tries_updated_total: Counter,
    /// Histogram of state root duration
    pub(crate) state_root_histogram: Histogram,
    /// Latest state root duration
    pub(crate) state_root_duration: Gauge,
    /// Trie input computation duration
    pub(crate) trie_input_duration: Histogram,
}

impl BlockValidationMetrics {
    /// Records a new state root time, updating both the histogram and state root gauge
    pub(crate) fn record_state_root(&self, trie_output: &TrieUpdates, elapsed_as_secs: f64) {
        self.state_root_storage_tries_updated_total
            .increment(trie_output.storage_tries_ref().len() as u64);
        self.state_root_duration.set(elapsed_as_secs);
        self.state_root_histogram.record(elapsed_as_secs);
    }
}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub(crate) struct BlockBufferMetrics {
    /// Total blocks in the block buffer
    pub blocks: Gauge,
}
