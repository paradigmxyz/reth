use crate::tree::MeteredStateHook;
use alloy_consensus::BlockHeader;
use alloy_evm::{
    block::{BlockExecutor, ExecutableTx},
    Evm,
};
use core::borrow::BorrowMut;
use reth_errors::BlockExecutionError;
use reth_evm::{metrics::ExecutorMetrics, Database, OnStateHook};
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_trie::updates::TrieUpdates;
use revm::database::{states::bundle_state::BundleRetention, State};
use std::time::Instant;

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
    pub tree: TreeMetrics,
}

impl EngineApiMetrics {
    /// Helper function for metered execution
    fn metered<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> (u64, R),
    {
        // Execute the block and record the elapsed time.
        let execute_start = Instant::now();
        let (gas_used, output) = f();
        let execution_duration = execute_start.elapsed().as_secs_f64();

        // Update gas metrics.
        self.executor.gas_processed_total.increment(gas_used);
        self.executor.gas_per_second.set(gas_used as f64 / execution_duration);
        self.executor.gas_used_histogram.record(gas_used as f64);
        self.executor.execution_histogram.record(execution_duration);
        self.executor.execution_duration.set(execution_duration);

        output
    }

    /// Execute the given block using the provided [`BlockExecutor`] and update metrics for the
    /// execution.
    ///
    /// This method updates metrics for execution time, gas usage, and the number
    /// of accounts, storage slots and bytecodes loaded and updated.
    pub(crate) fn execute_metered<E, DB>(
        &self,
        executor: E,
        transactions: impl Iterator<Item = Result<impl ExecutableTx<E>, BlockExecutionError>>,
        state_hook: Box<dyn OnStateHook>,
    ) -> Result<BlockExecutionOutput<E::Receipt>, BlockExecutionError>
    where
        DB: Database,
        E: BlockExecutor<Evm: Evm<DB: BorrowMut<State<DB>>>>,
    {
        // clone here is cheap, all the metrics are Option<Arc<_>>. additionally
        // they are globally registered so that the data recorded in the hook will
        // be accessible.
        let wrapper = MeteredStateHook { metrics: self.executor.clone(), inner_hook: state_hook };

        let mut executor = executor.with_state_hook(Some(Box::new(wrapper)));

        let f = || {
            executor.apply_pre_execution_changes()?;
            for tx in transactions {
                executor.execute_transaction(tx?)?;
            }
            executor.finish().map(|(evm, result)| (evm.into_db(), result))
        };

        // Use metered to execute and track timing/gas metrics
        let (mut db, result) = self.metered(|| {
            let res = f();
            let gas_used = res.as_ref().map(|r| r.1.gas_used).unwrap_or(0);
            (gas_used, res)
        })?;

        // merge transactions into bundle state
        db.borrow_mut().merge_transitions(BundleRetention::Reverts);
        let output = BlockExecutionOutput { result, state: db.borrow_mut().take_bundle() };

        // Update the metrics for the number of accounts, storage slots and bytecodes updated
        let accounts = output.state.state.len();
        let storage_slots =
            output.state.state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = output.state.contracts.len();

        self.executor.accounts_updated_histogram.record(accounts as f64);
        self.executor.storage_slots_updated_histogram.record(storage_slots as f64);
        self.executor.bytecodes_updated_histogram.record(bytecodes as f64);

        Ok(output)
    }
}

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub(crate) struct TreeMetrics {
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
    /// The total count of forkchoice updated messages with payload received.
    pub(crate) forkchoice_with_attributes_updated_messages: Counter,
    /// Newly arriving block hash is not present in executed blocks cache storage
    pub(crate) executed_new_block_cache_miss: Counter,
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
    /// Total number of times the parallel state root computation fell back to regular.
    pub(crate) state_root_parallel_fallback_total: Counter,
    /// Histogram of state root duration
    pub(crate) state_root_histogram: Histogram,
    /// Latest state root duration
    pub(crate) state_root_duration: Gauge,
    /// Trie input computation duration
    pub(crate) trie_input_duration: Histogram,
    /// Payload conversion and validation latency
    pub(crate) payload_validation_duration: Gauge,
    /// Histogram of payload validation latency
    pub(crate) payload_validation_histogram: Histogram,
}

impl BlockValidationMetrics {
    /// Records a new state root time, updating both the histogram and state root gauge
    pub(crate) fn record_state_root(&self, trie_output: &TrieUpdates, elapsed_as_secs: f64) {
        self.state_root_storage_tries_updated_total
            .increment(trie_output.storage_tries_ref().len() as u64);
        self.state_root_duration.set(elapsed_as_secs);
        self.state_root_histogram.record(elapsed_as_secs);
    }

    /// Records a new payload validation time, updating both the histogram and the payload
    /// validation gauge
    pub(crate) fn record_payload_validation(&self, elapsed_as_secs: f64) {
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
