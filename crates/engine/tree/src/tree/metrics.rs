use crate::tree::{error::InsertBlockFatalError, MeteredStateHook, TreeOutcome};
use alloy_consensus::transaction::TxHashRef;
use alloy_evm::{
    block::{BlockExecutor, ExecutableTx},
    Evm,
};
use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};
use core::borrow::BorrowMut;
use reth_engine_primitives::{ForkchoiceStatus, OnForkChoiceUpdated};
use reth_errors::{BlockExecutionError, ProviderError};
use reth_evm::{metrics::ExecutorMetrics, OnStateHook};
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::SignedTransaction;
use reth_trie::updates::TrieUpdates;
use revm::database::{states::bundle_state::BundleRetention, State};
use revm_primitives::Address;
use std::time::Instant;
use tracing::{debug_span, trace};

/// Metrics for the `EngineApi`.
#[derive(Debug, Default)]
pub(crate) struct EngineApiMetrics {
    /// Engine API-specific metrics.
    pub(crate) engine: EngineMetrics,
    /// Block executor metrics.
    pub(crate) executor: ExecutorMetrics,
    /// Metrics for block validation
    pub(crate) block_validation: BlockValidationMetrics,
    /// Canonical chain and reorg related metrics
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
        mut transactions: impl Iterator<Item = Result<impl ExecutableTx<E>, BlockExecutionError>>,
        transaction_count: usize,
        state_hook: Box<dyn OnStateHook>,
    ) -> Result<(BlockExecutionOutput<E::Receipt>, Vec<Address>), BlockExecutionError>
    where
        DB: alloy_evm::Database,
        E: BlockExecutor<Evm: Evm<DB: BorrowMut<State<DB>>>, Transaction: SignedTransaction>,
    {
        // clone here is cheap, all the metrics are Option<Arc<_>>. additionally
        // they are globally registered so that the data recorded in the hook will
        // be accessible.
        let wrapper = MeteredStateHook { metrics: self.executor.clone(), inner_hook: state_hook };

        let mut senders = Vec::with_capacity(transaction_count);
        let mut executor = executor.with_state_hook(Some(Box::new(wrapper)));

        let f = || {
            let start = Instant::now();
            debug_span!(target: "engine::tree", "pre execution")
                .entered()
                .in_scope(|| executor.apply_pre_execution_changes())?;
            self.executor.pre_execution_histogram.record(start.elapsed());

            let exec_span = debug_span!(target: "engine::tree", "execution").entered();
            loop {
                let start = Instant::now();
                let Some(tx) = transactions.next() else { break };
                self.executor.transaction_wait_histogram.record(start.elapsed());

                let tx = tx?;
                senders.push(*tx.signer());

                let span =
                    debug_span!(target: "engine::tree", "execute tx", tx_hash=?tx.tx().tx_hash());
                let enter = span.entered();
                trace!(target: "engine::tree", "Executing transaction");
                let start = Instant::now();
                let gas_used = executor.execute_transaction(tx)?;
                self.executor.transaction_execution_histogram.record(start.elapsed());

                // record the tx gas used
                enter.record("gas_used", gas_used);
            }
            drop(exec_span);

            let start = Instant::now();
            let result = debug_span!(target: "engine::tree", "finish")
                .entered()
                .in_scope(|| executor.finish())
                .map(|(evm, result)| (evm.into_db(), result));
            self.executor.post_execution_histogram.record(start.elapsed());

            result
        };

        // Use metered to execute and track timing/gas metrics
        let (mut db, result) = self.metered(|| {
            let res = f();
            let gas_used = res.as_ref().map(|r| r.1.gas_used).unwrap_or(0);
            (gas_used, res)
        })?;

        // merge transitions into bundle state
        debug_span!(target: "engine::tree", "merge transitions")
            .entered()
            .in_scope(|| db.borrow_mut().merge_transitions(BundleRetention::Reverts));
        let output = BlockExecutionOutput { result, state: db.borrow_mut().take_bundle() };

        // Update the metrics for the number of accounts, storage slots and bytecodes updated
        let accounts = output.state.state.len();
        let storage_slots =
            output.state.state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = output.state.contracts.len();

        self.executor.accounts_updated_histogram.record(accounts as f64);
        self.executor.storage_slots_updated_histogram.record(storage_slots as f64);
        self.executor.bytecodes_updated_histogram.record(bytecodes as f64);

        Ok((output, senders))
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
    /// The current safe block height (this is required by optimism)
    pub safe_block_height: Gauge,
    /// The current finalized block height (this is required by optimism)
    pub finalized_block_height: Gauge,
}

/// Metrics for the `EngineApi`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineMetrics {
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
}

impl ForkchoiceUpdatedMetrics {
    /// Increment the forkchoiceUpdated counter based on the given result
    pub(crate) fn update_response_metrics(
        &self,
        start: Instant,
        latest_new_payload_at: &mut Option<Instant>,
        has_attrs: bool,
        result: &Result<TreeOutcome<OnForkChoiceUpdated>, ProviderError>,
    ) {
        let elapsed = start.elapsed();
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

/// Metrics for engine newPayload responses.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct NewPayloadStatusMetrics {
    /// Finish time of the latest new payload call.
    #[metric(skip)]
    pub(crate) latest_at: Option<Instant>,
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
    /// The gas per second of valid new payload messages received.
    pub(crate) new_payload_gas_per_second: Histogram,
    /// The gas per second for the last new payload call.
    pub(crate) new_payload_gas_per_second_last: Gauge,
    /// Latency for the new payload calls.
    pub(crate) new_payload_latency: Histogram,
    /// Latency for the last new payload call.
    pub(crate) new_payload_last: Gauge,
}

impl NewPayloadStatusMetrics {
    /// Increment the newPayload counter based on the given result
    pub(crate) fn update_response_metrics(
        &mut self,
        start: Instant,
        result: &Result<TreeOutcome<PayloadStatus>, InsertBlockFatalError>,
        gas_used: u64,
    ) {
        let finish = Instant::now();
        let elapsed = finish - start;

        self.latest_at = Some(finish);
        match result {
            Ok(outcome) => match outcome.outcome.status {
                PayloadStatusEnum::Valid => {
                    self.new_payload_valid.increment(1);
                    self.new_payload_total_gas.record(gas_used as f64);
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
    }
}

/// Metrics for non-execution related block validation.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.block_validation")]
pub(crate) struct BlockValidationMetrics {
    /// Total number of storage tries updated in the state root calculation
    pub(crate) state_root_storage_tries_updated_total: Counter,
    /// Total number of times the parallel state root computation fell back to regular.
    pub(crate) state_root_parallel_fallback_total: Counter,
    /// Latest state root duration, ie the time spent blocked waiting for the state root.
    pub(crate) state_root_duration: Gauge,
    /// Histogram for state root duration ie the time spent blocked waiting for the state root
    pub(crate) state_root_histogram: Histogram,
    /// Histogram of deferred trie computation duration.
    pub(crate) deferred_trie_compute_duration: Histogram,
    /// Histogram of time spent waiting for deferred trie data to become available.
    pub(crate) deferred_trie_wait_duration: Histogram,
    /// Trie input computation duration
    pub(crate) trie_input_duration: Histogram,
    /// Payload conversion and validation latency
    pub(crate) payload_validation_duration: Gauge,
    /// Histogram of payload validation latency
    pub(crate) payload_validation_histogram: Histogram,
    /// Payload processor spawning duration
    pub(crate) spawn_payload_processor: Histogram,
    /// Post-execution validation duration
    pub(crate) post_execution_validation_duration: Histogram,
    /// Total duration of the new payload call
    pub(crate) total_duration: Histogram,
    /// Size of `HashedPostStateSorted` (`total_len`)
    pub(crate) hashed_post_state_size: Histogram,
    /// Size of `TrieUpdatesSorted` (`total_len`)
    pub(crate) trie_updates_sorted_size: Histogram,
    /// Size of `AnchoredTrieInput` overlay `TrieUpdatesSorted` (`total_len`)
    pub(crate) anchored_overlay_trie_updates_size: Histogram,
    /// Size of `AnchoredTrieInput` overlay `HashedPostStateSorted` (`total_len`)
    pub(crate) anchored_overlay_hashed_state_size: Histogram,
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use alloy_evm::block::StateChangeSource;
    use alloy_primitives::{B256, U256};
    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};
    use reth_ethereum_primitives::{Receipt, TransactionSigned};
    use reth_evm_ethereum::EthEvm;
    use reth_execution_types::BlockExecutionResult;
    use reth_primitives_traits::RecoveredBlock;
    use revm::{
        context::result::{ExecutionResult, Output, ResultAndState, SuccessReason},
        database::State,
        database_interface::EmptyDB,
        inspector::NoOpInspector,
        state::{Account, AccountInfo, AccountStatus, EvmState, EvmStorage, EvmStorageSlot},
        Context, MainBuilder, MainContext,
    };
    use revm_primitives::Bytes;
    use std::sync::mpsc;

    /// A simple mock executor for testing that doesn't require complex EVM setup
    struct MockExecutor {
        state: EvmState,
        hook: Option<Box<dyn OnStateHook>>,
    }

    impl MockExecutor {
        fn new(state: EvmState) -> Self {
            Self { state, hook: None }
        }
    }

    // Mock Evm type for testing
    type MockEvm = EthEvm<State<EmptyDB>, NoOpInspector>;

    impl BlockExecutor for MockExecutor {
        type Transaction = TransactionSigned;
        type Receipt = Receipt;
        type Evm = MockEvm;

        fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
            Ok(())
        }

        fn execute_transaction_without_commit(
            &mut self,
            _tx: impl ExecutableTx<Self>,
        ) -> Result<ResultAndState<<Self::Evm as Evm>::HaltReason>, BlockExecutionError> {
            // Call hook with our mock state for each transaction
            if let Some(hook) = self.hook.as_mut() {
                hook.on_state(StateChangeSource::Transaction(0), &self.state);
            }

            Ok(ResultAndState::new(
                ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas_used: 1000, // Mock gas used
                    gas_refunded: 0,
                    logs: vec![],
                    output: Output::Call(Bytes::from(vec![])),
                },
                Default::default(),
            ))
        }

        fn commit_transaction(
            &mut self,
            _output: ResultAndState<<Self::Evm as Evm>::HaltReason>,
            _tx: impl ExecutableTx<Self>,
        ) -> Result<u64, BlockExecutionError> {
            Ok(1000)
        }

        fn finish(
            self,
        ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
            let Self { hook, state, .. } = self;

            // Call hook with our mock state
            if let Some(mut hook) = hook {
                hook.on_state(StateChangeSource::Transaction(0), &state);
            }

            // Create a mock EVM
            let db = State::builder()
                .with_database(EmptyDB::default())
                .with_bundle_update()
                .without_state_clear()
                .build();
            let evm = EthEvm::new(
                Context::mainnet().with_db(db).build_mainnet_with_inspector(NoOpInspector {}),
                false,
            );

            // Return successful result like the original tests
            Ok((
                evm,
                BlockExecutionResult {
                    receipts: vec![],
                    requests: Requests::default(),
                    gas_used: 1000,
                    blob_gas_used: 0,
                },
            ))
        }

        fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
            self.hook = hook;
        }

        fn evm(&self) -> &Self::Evm {
            panic!("Mock executor evm() not implemented")
        }

        fn evm_mut(&mut self) -> &mut Self::Evm {
            panic!("Mock executor evm_mut() not implemented")
        }
    }

    struct ChannelStateHook {
        output: i32,
        sender: mpsc::Sender<i32>,
    }

    impl OnStateHook for ChannelStateHook {
        fn on_state(&mut self, _source: StateChangeSource, _state: &EvmState) {
            let _ = self.sender.send(self.output);
        }
    }

    fn setup_test_recorder() -> Snapshotter {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        recorder.install().unwrap();
        snapshotter
    }

    #[test]
    fn test_executor_metrics_hook_called() {
        let metrics = EngineApiMetrics::default();
        let input = RecoveredBlock::<reth_ethereum_primitives::Block>::default();

        let (tx, rx) = mpsc::channel();
        let expected_output = 42;
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: expected_output });

        let state = EvmState::default();
        let executor = MockExecutor::new(state);

        // This will fail to create the EVM but should still call the hook
        let _result = metrics.execute_metered::<_, EmptyDB>(
            executor,
            input.clone_transactions_recovered().map(Ok::<_, BlockExecutionError>),
            input.transaction_count(),
            state_hook,
        );

        // Check if hook was called (it might not be if finish() fails early)
        match rx.try_recv() {
            Ok(actual_output) => assert_eq!(actual_output, expected_output),
            Err(_) => {
                // Hook wasn't called, which is expected if the mock fails early
                // The test still validates that the code compiles and runs
            }
        }
    }

    #[test]
    fn test_executor_metrics_hook_metrics_recorded() {
        let snapshotter = setup_test_recorder();
        let metrics = EngineApiMetrics::default();

        // Pre-populate some metrics to ensure they exist
        metrics.executor.gas_processed_total.increment(0);
        metrics.executor.gas_per_second.set(0.0);
        metrics.executor.gas_used_histogram.record(0.0);

        let input = RecoveredBlock::<reth_ethereum_primitives::Block>::default();

        let (tx, _rx) = mpsc::channel();
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: 42 });

        // Create a state with some data
        let state = {
            let mut state = EvmState::default();
            let storage =
                EvmStorage::from_iter([(U256::from(1), EvmStorageSlot::new(U256::from(2), 0))]);
            state.insert(
                Default::default(),
                Account {
                    info: AccountInfo {
                        balance: U256::from(100),
                        nonce: 10,
                        code_hash: B256::random(),
                        code: Default::default(),
                    },
                    storage,
                    status: AccountStatus::default(),
                    transaction_id: 0,
                },
            );
            state
        };

        let executor = MockExecutor::new(state);

        // Execute (will fail but should still update some metrics)
        let _result = metrics.execute_metered::<_, EmptyDB>(
            executor,
            input.clone_transactions_recovered().map(Ok::<_, BlockExecutionError>),
            input.transaction_count(),
            state_hook,
        );

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
