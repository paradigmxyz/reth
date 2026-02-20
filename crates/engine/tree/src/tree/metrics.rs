use crate::tree::{error::InsertBlockFatalError, TreeOutcome};
use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};
use reth_engine_primitives::{ForkchoiceStatus, OnForkChoiceUpdated};
use reth_errors::ProviderError;
use reth_evm::metrics::ExecutorMetrics;
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::{constants::gas_units::MEGAGAS, FastInstant as Instant};
use reth_trie::updates::TrieUpdates;
use std::time::Duration;

/// Upper bounds for each gas bucket. The last bucket is a catch-all for
/// everything above the final threshold: <5M, 5-10M, 10-20M, 20-30M, 30-40M, >40M.
const GAS_BUCKET_THRESHOLDS: [u64; 5] =
    [5 * MEGAGAS, 10 * MEGAGAS, 20 * MEGAGAS, 30 * MEGAGAS, 40 * MEGAGAS];

/// Total number of gas buckets (thresholds + 1 catch-all).
const NUM_GAS_BUCKETS: usize = GAS_BUCKET_THRESHOLDS.len() + 1;

/// Metrics for the `EngineApi`.
#[derive(Debug, Default)]
pub struct EngineApiMetrics {
    /// Engine API-specific metrics.
    pub engine: EngineMetrics,
    /// Block executor metrics.
    pub executor: ExecutorMetrics,
    /// Metrics for block validation
    pub block_validation: BlockValidationMetrics,
    /// Canonical chain and reorg related metrics
    pub tree: TreeMetrics,
    /// Metrics for EIP-7928 Block-Level Access Lists (BAL).
    #[allow(dead_code)]
    pub(crate) bal: BalMetrics,
    /// Gas-bucketed execution sub-phase metrics.
    pub(crate) execution_gas_buckets: ExecutionGasBucketMetrics,
    /// Gas-bucketed block validation sub-phase metrics.
    pub(crate) block_validation_gas_buckets: BlockValidationGasBucketMetrics,
}

impl EngineApiMetrics {
    /// Records metrics for block execution.
    ///
    /// This method updates metrics for execution time, gas usage, and the number
    /// of accounts, storage slots and bytecodes updated.
    pub fn record_block_execution<R>(
        &self,
        output: &BlockExecutionOutput<R>,
        execution_duration: Duration,
    ) {
        let execution_secs = execution_duration.as_secs_f64();
        let gas_used = output.result.gas_used;

        // Update gas metrics
        self.executor.gas_processed_total.increment(gas_used);
        self.executor.gas_per_second.set(gas_used as f64 / execution_secs);
        self.executor.gas_used_histogram.record(gas_used as f64);
        self.executor.execution_histogram.record(execution_secs);
        self.executor.execution_duration.set(execution_secs);

        // Update the metrics for the number of accounts, storage slots and bytecodes
        let accounts = output.state.state.len();
        let storage_slots =
            output.state.state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = output.state.contracts.len();

        self.executor.accounts_updated_histogram.record(accounts as f64);
        self.executor.storage_slots_updated_histogram.record(storage_slots as f64);
        self.executor.bytecodes_updated_histogram.record(bytecodes as f64);
    }

    /// Returns a reference to the executor metrics for use in state hooks.
    pub const fn executor_metrics(&self) -> &ExecutorMetrics {
        &self.executor
    }

    /// Records the duration of block pre-execution changes (e.g., beacon root update).
    pub fn record_pre_execution(&self, elapsed: Duration) {
        self.executor.pre_execution_histogram.record(elapsed);
    }

    /// Records the duration of block post-execution changes (e.g., finalization).
    pub fn record_post_execution(&self, elapsed: Duration) {
        self.executor.post_execution_histogram.record(elapsed);
    }

    /// Records execution duration into the gas-bucketed execution histogram.
    pub fn record_block_execution_gas_bucket(&self, gas_used: u64, elapsed: Duration) {
        let idx = GasBucketMetrics::bucket_index(gas_used);
        self.execution_gas_buckets.buckets[idx]
            .execution_gas_bucket_histogram
            .record(elapsed.as_secs_f64());
    }

    /// Records state root duration into the gas-bucketed block validation histogram.
    pub fn record_state_root_gas_bucket(&self, gas_used: u64, elapsed_secs: f64) {
        let idx = GasBucketMetrics::bucket_index(gas_used);
        self.block_validation_gas_buckets.buckets[idx]
            .state_root_gas_bucket_histogram
            .record(elapsed_secs);
    }

    /// Records the time spent waiting for the next transaction from the iterator.
    pub fn record_transaction_wait(&self, elapsed: Duration) {
        self.executor.transaction_wait_histogram.record(elapsed);
    }

    /// Records the duration of a single transaction execution.
    pub fn record_transaction_execution(&self, elapsed: Duration) {
        self.executor.transaction_execution_histogram.record(elapsed);
    }
}

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub struct TreeMetrics {
    /// The highest block number in the canonical chain
    pub canonical_chain_height: Gauge,
    /// Metrics for reorgs.
    #[metric(skip)]
    pub reorgs: ReorgMetrics,
    /// The latest reorg depth
    pub latest_reorg_depth: Gauge,
    /// The current safe block height (this is required by optimism)
    pub safe_block_height: Gauge,
    /// The current finalized block height (this is required by optimism)
    pub finalized_block_height: Gauge,
}

/// Metrics for reorgs.
#[derive(Debug)]
pub struct ReorgMetrics {
    /// The number of head block reorgs
    pub head: Counter,
    /// The number of safe block reorgs
    pub safe: Counter,
    /// The number of finalized block reorgs
    pub finalized: Counter,
}

impl Default for ReorgMetrics {
    fn default() -> Self {
        Self {
            head: metrics::counter!("blockchain_tree_reorgs", "commitment" => "head"),
            safe: metrics::counter!("blockchain_tree_reorgs", "commitment" => "safe"),
            finalized: metrics::counter!("blockchain_tree_reorgs", "commitment" => "finalized"),
        }
    }
}

/// Metrics for the `EngineApi`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub struct EngineMetrics {
    /// Engine API forkchoiceUpdated response type metrics
    #[metric(skip)]
    pub(crate) forkchoice_updated: ForkchoiceUpdatedMetrics,
    /// Engine API newPayload response type metrics
    #[metric(skip)]
    pub(crate) new_payload: NewPayloadStatusMetrics,
    /// How many executed blocks are currently stored.
    pub(crate) executed_blocks: Gauge,
    /// How many already executed blocks were directly inserted into the tree.
    pub(crate) inserted_already_executed_blocks: Counter,
    /// The number of times the pipeline was run.
    pub(crate) pipeline_runs: Counter,
    /// Newly arriving block hash is not present in executed blocks cache storage
    pub(crate) executed_new_block_cache_miss: Counter,
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
    /// block insert duration
    pub(crate) block_insert_total_duration: Histogram,
}

/// Metrics for engine forkchoiceUpdated responses.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct ForkchoiceUpdatedMetrics {
    /// Finish time of the latest forkchoice updated call.
    #[metric(skip)]
    pub(crate) latest_finish_at: Option<Instant>,
    /// Start time of the latest forkchoice updated call.
    #[metric(skip)]
    pub(crate) latest_start_at: Option<Instant>,
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of forkchoice updated messages with payload received.
    pub(crate) forkchoice_with_attributes_updated_messages: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Valid`](ForkchoiceStatus::Valid).
    pub(crate) forkchoice_updated_valid: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Invalid`](ForkchoiceStatus::Invalid).
    pub(crate) forkchoice_updated_invalid: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Syncing`](ForkchoiceStatus::Syncing).
    pub(crate) forkchoice_updated_syncing: Counter,
    /// The total count of forkchoice updated messages that were unsuccessful, i.e. we responded
    /// with an error type that is not a [`PayloadStatusEnum`].
    pub(crate) forkchoice_updated_error: Counter,
    /// Latency for the forkchoice updated calls.
    pub(crate) forkchoice_updated_latency: Histogram,
    /// Latency for the last forkchoice updated call.
    pub(crate) forkchoice_updated_last: Gauge,
    /// Time diff between new payload call response and the next forkchoice updated call request.
    pub(crate) new_payload_forkchoice_updated_time_diff: Histogram,
    /// Time from previous forkchoice updated finish to current forkchoice updated start (idle
    /// time).
    pub(crate) time_between_forkchoice_updated: Histogram,
    /// Time from previous forkchoice updated start to current forkchoice updated start (total
    /// interval).
    pub(crate) forkchoice_updated_interval: Histogram,
}

impl ForkchoiceUpdatedMetrics {
    /// Increment the forkchoiceUpdated counter based on the given result
    pub(crate) fn update_response_metrics(
        &mut self,
        start: Instant,
        latest_new_payload_at: &mut Option<Instant>,
        has_attrs: bool,
        result: &Result<TreeOutcome<OnForkChoiceUpdated>, ProviderError>,
    ) {
        let finish = Instant::now();
        let elapsed = finish - start;

        if let Some(prev_finish) = self.latest_finish_at {
            self.time_between_forkchoice_updated.record(start - prev_finish);
        }
        if let Some(prev_start) = self.latest_start_at {
            self.forkchoice_updated_interval.record(start - prev_start);
        }
        self.latest_finish_at = Some(finish);
        self.latest_start_at = Some(start);

        match result {
            Ok(outcome) => match outcome.outcome.forkchoice_status() {
                ForkchoiceStatus::Valid => self.forkchoice_updated_valid.increment(1),
                ForkchoiceStatus::Invalid => self.forkchoice_updated_invalid.increment(1),
                ForkchoiceStatus::Syncing => self.forkchoice_updated_syncing.increment(1),
            },
            Err(_) => self.forkchoice_updated_error.increment(1),
        }
        self.forkchoice_updated_messages.increment(1);
        if has_attrs {
            self.forkchoice_with_attributes_updated_messages.increment(1);
        }
        self.forkchoice_updated_latency.record(elapsed);
        self.forkchoice_updated_last.set(elapsed);
        if let Some(latest_new_payload_at) = latest_new_payload_at.take() {
            self.new_payload_forkchoice_updated_time_diff.record(start - latest_new_payload_at);
        }
    }
}

/// Per-gas-bucket newPayload metrics, initialized once via [`Self::new_with_labels`].
#[derive(Clone, Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct NewPayloadGasBucketMetrics {
    /// Latency for new payload calls in this gas bucket.
    pub(crate) new_payload_gas_bucket_latency: Histogram,
    /// Gas per second for new payload calls in this gas bucket.
    pub(crate) new_payload_gas_bucket_gas_per_second: Histogram,
}

/// Holds pre-initialized [`NewPayloadGasBucketMetrics`] instances, one per gas bucket.
#[derive(Debug)]
pub(crate) struct GasBucketMetrics {
    buckets: [NewPayloadGasBucketMetrics; NUM_GAS_BUCKETS],
}

impl Default for GasBucketMetrics {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|i| {
                let label = Self::bucket_label(i);
                NewPayloadGasBucketMetrics::new_with_labels(&[("gas_bucket", label)])
            }),
        }
    }
}

impl GasBucketMetrics {
    fn record(&self, gas_used: u64, elapsed: Duration) {
        let idx = Self::bucket_index(gas_used);
        self.buckets[idx].new_payload_gas_bucket_latency.record(elapsed);
        self.buckets[idx]
            .new_payload_gas_bucket_gas_per_second
            .record(gas_used as f64 / elapsed.as_secs_f64());
    }

    /// Returns the bucket index for a given gas value.
    pub(crate) fn bucket_index(gas_used: u64) -> usize {
        GAS_BUCKET_THRESHOLDS
            .iter()
            .position(|&threshold| gas_used < threshold)
            .unwrap_or(GAS_BUCKET_THRESHOLDS.len())
    }

    /// Returns a human-readable label like `<5M`, `5-10M`, … `>40M`.
    pub(crate) fn bucket_label(index: usize) -> String {
        if index == 0 {
            let hi = GAS_BUCKET_THRESHOLDS[0] / MEGAGAS;
            format!("<{hi}M")
        } else if index < GAS_BUCKET_THRESHOLDS.len() {
            let lo = GAS_BUCKET_THRESHOLDS[index - 1] / MEGAGAS;
            let hi = GAS_BUCKET_THRESHOLDS[index] / MEGAGAS;
            format!("{lo}-{hi}M")
        } else {
            let lo = GAS_BUCKET_THRESHOLDS[GAS_BUCKET_THRESHOLDS.len() - 1] / MEGAGAS;
            format!(">{lo}M")
        }
    }
}

/// Per-gas-bucket execution duration metric.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.execution")]
pub(crate) struct ExecutionGasBucketSeries {
    /// Gas-bucketed EVM execution duration.
    pub(crate) execution_gas_bucket_histogram: Histogram,
}

/// Holds pre-initialized [`ExecutionGasBucketSeries`] instances, one per gas bucket.
#[derive(Debug)]
pub(crate) struct ExecutionGasBucketMetrics {
    buckets: [ExecutionGasBucketSeries; NUM_GAS_BUCKETS],
}

impl Default for ExecutionGasBucketMetrics {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|i| {
                let label = GasBucketMetrics::bucket_label(i);
                ExecutionGasBucketSeries::new_with_labels(&[("gas_bucket", label)])
            }),
        }
    }
}

/// Per-gas-bucket block validation metrics (state root).
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.block_validation")]
pub(crate) struct BlockValidationGasBucketSeries {
    /// Gas-bucketed state root computation duration.
    pub(crate) state_root_gas_bucket_histogram: Histogram,
}

/// Holds pre-initialized [`BlockValidationGasBucketSeries`] instances, one per gas bucket.
#[derive(Debug)]
pub(crate) struct BlockValidationGasBucketMetrics {
    buckets: [BlockValidationGasBucketSeries; NUM_GAS_BUCKETS],
}

impl Default for BlockValidationGasBucketMetrics {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|i| {
                let label = GasBucketMetrics::bucket_label(i);
                BlockValidationGasBucketSeries::new_with_labels(&[("gas_bucket", label)])
            }),
        }
    }
}

/// Metrics for engine newPayload responses.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct NewPayloadStatusMetrics {
    /// Finish time of the latest new payload call.
    #[metric(skip)]
    pub(crate) latest_finish_at: Option<Instant>,
    /// Start time of the latest new payload call.
    #[metric(skip)]
    pub(crate) latest_start_at: Option<Instant>,
    /// Gas-bucket-labeled latency and gas/s histograms.
    #[metric(skip)]
    pub(crate) gas_bucket: GasBucketMetrics,
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Valid](PayloadStatusEnum::Valid).
    pub(crate) new_payload_valid: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Invalid](PayloadStatusEnum::Invalid).
    pub(crate) new_payload_invalid: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Syncing](PayloadStatusEnum::Syncing).
    pub(crate) new_payload_syncing: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Accepted](PayloadStatusEnum::Accepted).
    pub(crate) new_payload_accepted: Counter,
    /// The total count of new payload messages that were unsuccessful, i.e. we responded with an
    /// error type that is not a [`PayloadStatusEnum`].
    pub(crate) new_payload_error: Counter,
    /// The total gas of valid new payload messages received.
    pub(crate) new_payload_total_gas: Histogram,
    /// The gas used for the last valid new payload.
    pub(crate) new_payload_total_gas_last: Gauge,
    /// The gas per second of valid new payload messages received.
    pub(crate) new_payload_gas_per_second: Histogram,
    /// The gas per second for the last new payload call.
    pub(crate) new_payload_gas_per_second_last: Gauge,
    /// Latency for the new payload calls.
    pub(crate) new_payload_latency: Histogram,
    /// Latency for the last new payload call.
    pub(crate) new_payload_last: Gauge,
    /// Time from previous payload finish to current payload start (idle time).
    pub(crate) time_between_new_payloads: Histogram,
    /// Time from previous payload start to current payload start (total interval).
    pub(crate) new_payload_interval: Histogram,
    /// Time diff between forkchoice updated call response and the next new payload call request.
    pub(crate) forkchoice_updated_new_payload_time_diff: Histogram,
}

impl NewPayloadStatusMetrics {
    /// Increment the newPayload counter based on the given result
    pub(crate) fn update_response_metrics(
        &mut self,
        start: Instant,
        latest_forkchoice_updated_at: &mut Option<Instant>,
        result: &Result<TreeOutcome<PayloadStatus>, InsertBlockFatalError>,
        gas_used: u64,
    ) {
        let finish = Instant::now();
        let elapsed = finish - start;

        if let Some(prev_finish) = self.latest_finish_at {
            self.time_between_new_payloads.record(start - prev_finish);
        }
        if let Some(prev_start) = self.latest_start_at {
            self.new_payload_interval.record(start - prev_start);
        }
        self.latest_finish_at = Some(finish);
        self.latest_start_at = Some(start);
        match result {
            Ok(outcome) => match outcome.outcome.status {
                PayloadStatusEnum::Valid => {
                    self.new_payload_valid.increment(1);
                    self.new_payload_total_gas.record(gas_used as f64);
                    self.new_payload_total_gas_last.set(gas_used as f64);
                    let gas_per_second = gas_used as f64 / elapsed.as_secs_f64();
                    self.new_payload_gas_per_second.record(gas_per_second);
                    self.new_payload_gas_per_second_last.set(gas_per_second);
                }
                PayloadStatusEnum::Syncing => self.new_payload_syncing.increment(1),
                PayloadStatusEnum::Accepted => self.new_payload_accepted.increment(1),
                PayloadStatusEnum::Invalid { .. } => self.new_payload_invalid.increment(1),
            },
            Err(_) => self.new_payload_error.increment(1),
        }
        self.new_payload_messages.increment(1);
        self.new_payload_latency.record(elapsed);
        self.new_payload_last.set(elapsed);
        self.gas_bucket.record(gas_used, elapsed);
        if let Some(latest_forkchoice_updated_at) = latest_forkchoice_updated_at.take() {
            self.forkchoice_updated_new_payload_time_diff
                .record(start - latest_forkchoice_updated_at);
        }
    }
}

/// Metrics for EIP-7928 Block-Level Access Lists (BAL).
///
/// See also <https://github.com/ethereum/execution-metrics/issues/5>
#[allow(dead_code)]
#[derive(Metrics, Clone)]
#[metrics(scope = "execution.block_access_list")]
pub(crate) struct BalMetrics {
    /// Size of the BAL in bytes for the current block.
    pub(crate) size_bytes: Gauge,
    /// Total number of blocks with valid BALs.
    pub(crate) valid_total: Counter,
    /// Total number of blocks with invalid BALs.
    pub(crate) invalid_total: Counter,
    /// Time taken to validate the BAL against actual execution.
    pub(crate) validation_time_seconds: Histogram,
    /// Number of account changes in the BAL.
    pub(crate) account_changes: Gauge,
    /// Number of storage changes in the BAL.
    pub(crate) storage_changes: Gauge,
    /// Number of balance changes in the BAL.
    pub(crate) balance_changes: Gauge,
    /// Number of nonce changes in the BAL.
    pub(crate) nonce_changes: Gauge,
    /// Number of code changes in the BAL.
    pub(crate) code_changes: Gauge,
}

/// Metrics for non-execution related block validation.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.block_validation")]
pub struct BlockValidationMetrics {
    /// Total number of storage tries updated in the state root calculation
    pub state_root_storage_tries_updated_total: Counter,
    /// Total number of times the parallel state root computation fell back to regular.
    pub state_root_parallel_fallback_total: Counter,
    /// Total number of times the state root task failed but the fallback succeeded.
    pub state_root_task_fallback_success_total: Counter,
    /// Total number of times the state root task timed out and a sequential fallback was spawned.
    pub state_root_task_timeout_total: Counter,
    /// Latest state root duration, ie the time spent blocked waiting for the state root.
    pub state_root_duration: Gauge,
    /// Histogram for state root duration ie the time spent blocked waiting for the state root
    pub state_root_histogram: Histogram,
    /// Histogram of deferred trie computation duration.
    pub deferred_trie_compute_duration: Histogram,
    /// Payload conversion and validation latency
    pub payload_validation_duration: Gauge,
    /// Histogram of payload validation latency
    pub payload_validation_histogram: Histogram,
    /// Payload processor spawning duration
    pub spawn_payload_processor: Histogram,
    /// Post-execution validation duration
    pub post_execution_validation_duration: Histogram,
    /// Total duration of the new payload call
    pub total_duration: Histogram,
    /// Size of `HashedPostStateSorted` (`total_len`)
    pub hashed_post_state_size: Histogram,
    /// Size of `TrieUpdatesSorted` (`total_len`)
    pub trie_updates_sorted_size: Histogram,
    /// Size of `AnchoredTrieInput` overlay `TrieUpdatesSorted` (`total_len`)
    pub anchored_overlay_trie_updates_size: Histogram,
    /// Size of `AnchoredTrieInput` overlay `HashedPostStateSorted` (`total_len`)
    pub anchored_overlay_hashed_state_size: Histogram,
    /// Histogram of cached trie node count in `StoragesTrie` per modified account.
    /// 0 means no on-disk intermediate nodes exist for this account's storage trie.
    pub storage_trie_cached_nodes: Histogram,
    /// Histogram of changed storage slot count for accounts with zero cached trie nodes.
    pub changed_slots_when_no_cached_nodes: Histogram,
}

impl BlockValidationMetrics {
    /// Records a new state root time, updating both the histogram and state root gauge
    pub fn record_state_root(&self, trie_output: &TrieUpdates, elapsed_as_secs: f64) {
        self.state_root_storage_tries_updated_total
            .increment(trie_output.storage_tries_ref().len() as u64);
        self.state_root_duration.set(elapsed_as_secs);
        self.state_root_histogram.record(elapsed_as_secs);
    }

    /// Records a new payload validation time, updating both the histogram and the payload
    /// validation gauge
    pub fn record_payload_validation(&self, elapsed_as_secs: f64) {
        self.payload_validation_duration.set(elapsed_as_secs);
        self.payload_validation_histogram.record(elapsed_as_secs);
    }
}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub(crate) struct BlockBufferMetrics {
    /// Total blocks in the block buffer
    pub blocks: Gauge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};
    use reth_ethereum_primitives::Receipt;
    use reth_execution_types::BlockExecutionResult;
    use reth_revm::db::BundleState;

    fn setup_test_recorder() -> Snapshotter {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        recorder.install().unwrap();
        snapshotter
    }

    #[test]
    fn test_record_block_execution_metrics() {
        let snapshotter = setup_test_recorder();
        let metrics = EngineApiMetrics::default();

        // Pre-populate some metrics to ensure they exist
        metrics.executor.gas_processed_total.increment(0);
        metrics.executor.gas_per_second.set(0.0);
        metrics.executor.gas_used_histogram.record(0.0);

        let output = BlockExecutionOutput::<Receipt> {
            state: BundleState::default(),
            result: BlockExecutionResult {
                receipts: vec![],
                requests: Requests::default(),
                gas_used: 21000,
                blob_gas_used: 0,
            },
        };

        metrics.record_block_execution(&output, Duration::from_millis(100));

        let snapshot = snapshotter.snapshot().into_vec();

        // Verify that metrics were registered
        let mut found_metrics = false;
        for (key, _unit, _desc, _value) in snapshot {
            let metric_name = key.key().name();
            if metric_name.starts_with("sync.execution") {
                found_metrics = true;
                break;
            }
        }

        assert!(found_metrics, "Expected to find sync.execution metrics");
    }
}
