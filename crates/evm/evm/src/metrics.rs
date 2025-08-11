//! Executor metrics.
//!
//! Block processing related to syncing should take care to update the metrics by using either
//! [`ExecutorMetrics::execute_metered`] or [`ExecutorMetrics::metered_one`].
use crate::{Database, OnStateHook};
use alloy_consensus::BlockHeader;
use alloy_evm::{
    block::{BlockExecutor, ExecutableTx, StateChangeSource},
    Evm,
};
use core::borrow::BorrowMut;
use metrics::{Counter, Gauge, Histogram};
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::Metrics;
use reth_primitives_traits::RecoveredBlock;
use revm::{
    database::{states::bundle_state::BundleRetention, State},
    state::EvmState,
};
use std::time::Instant;

/// Wrapper struct that combines metrics and state hook
struct MeteredStateHook {
    metrics: ExecutorMetrics,
    inner_hook: Box<dyn OnStateHook>,
}

impl OnStateHook for MeteredStateHook {
    fn on_state(&mut self, source: StateChangeSource, state: &EvmState) {
        // Update the metrics for the number of accounts, storage slots and bytecodes loaded
        let accounts = state.keys().len();
        let storage_slots = state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = state.values().filter(|account| !account.info.is_empty_code_hash()).count();

        self.metrics.accounts_loaded_histogram.record(accounts as f64);
        self.metrics.storage_slots_loaded_histogram.record(storage_slots as f64);
        self.metrics.bytecodes_loaded_histogram.record(bytecodes as f64);

        // Call the original state hook
        self.inner_hook.on_state(source, state);
    }
}

/// Executor metrics.
// TODO(onbjerg): add sload/sstore
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.execution")]
pub struct ExecutorMetrics {
    /// The total amount of gas processed.
    pub gas_processed_total: Counter,
    /// The instantaneous amount of gas processed per second.
    pub gas_per_second: Gauge,
    /// The Histogram for amount of gas used.
    pub gas_used_histogram: Histogram,

    /// The Histogram for amount of time taken to execute blocks.
    pub execution_histogram: Histogram,
    /// The total amount of time it took to execute the latest block.
    pub execution_duration: Gauge,

    /// The Histogram for number of accounts loaded when executing the latest block.
    pub accounts_loaded_histogram: Histogram,
    /// The Histogram for number of storage slots loaded when executing the latest block.
    pub storage_slots_loaded_histogram: Histogram,
    /// The Histogram for number of bytecodes loaded when executing the latest block.
    pub bytecodes_loaded_histogram: Histogram,

    /// The Histogram for number of accounts updated when executing the latest block.
    pub accounts_updated_histogram: Histogram,
    /// The Histogram for number of storage slots updated when executing the latest block.
    pub storage_slots_updated_histogram: Histogram,
    /// The Histogram for number of bytecodes updated when executing the latest block.
    pub bytecodes_updated_histogram: Histogram,
}

impl ExecutorMetrics {
    fn metered<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> (u64, R),
    {
        // Execute the block and record the elapsed time.
        let execute_start = Instant::now();
        let (gas_used, output) = f();
        let execution_duration = execute_start.elapsed().as_secs_f64();

        // Update gas metrics.
        self.gas_processed_total.increment(gas_used);
        self.gas_per_second.set(gas_used as f64 / execution_duration);
        self.gas_used_histogram.record(gas_used as f64);
        self.execution_histogram.record(execution_duration);
        self.execution_duration.set(execution_duration);

        output
    }

    /// Execute the given block using the provided [`BlockExecutor`] and update metrics for the
    /// execution.
    ///
    /// Compared to [`Self::metered_one`], this method additionally updates metrics for the number
    /// of accounts, storage slots and bytecodes loaded and updated.
    /// Execute the given block using the provided [`BlockExecutor`] and update metrics for the
    /// execution.
    pub fn execute_metered<E, DB>(
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
        let wrapper = MeteredStateHook { metrics: self.clone(), inner_hook: state_hook };

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

        self.accounts_updated_histogram.record(accounts as f64);
        self.storage_slots_updated_histogram.record(storage_slots as f64);
        self.bytecodes_updated_histogram.record(bytecodes as f64);

        Ok(output)
    }

    /// Execute the given block and update metrics for the execution.
    pub fn metered_one<F, R, B>(&self, input: &RecoveredBlock<B>, f: F) -> R
    where
        F: FnOnce(&RecoveredBlock<B>) -> R,
        B: reth_primitives_traits::Block,
    {
        self.metered(|| (input.header().gas_used(), f(input)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use alloy_evm::{block::CommitChanges, EthEvm};
    use alloy_primitives::{B256, U256};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use reth_ethereum_primitives::{Receipt, TransactionSigned};
    use reth_execution_types::BlockExecutionResult;
    use revm::{
        context::result::ExecutionResult,
        database::State,
        database_interface::EmptyDB,
        inspector::NoOpInspector,
        state::{Account, AccountInfo, AccountStatus, EvmStorage, EvmStorageSlot},
        Context, MainBuilder, MainContext,
    };
    use std::sync::mpsc;

    /// A mock executor that simulates state changes
    struct MockExecutor {
        state: EvmState,
        hook: Option<Box<dyn OnStateHook>>,
        evm: EthEvm<State<EmptyDB>, NoOpInspector>,
    }

    impl MockExecutor {
        fn new(state: EvmState) -> Self {
            let db = State::builder()
                .with_database(EmptyDB::default())
                .with_bundle_update()
                .without_state_clear()
                .build();
            let evm = EthEvm::new(
                Context::mainnet().with_db(db).build_mainnet_with_inspector(NoOpInspector {}),
                false,
            );
            Self { state, hook: None, evm }
        }
    }

    impl BlockExecutor for MockExecutor {
        type Transaction = TransactionSigned;
        type Receipt = Receipt;
        type Evm = EthEvm<State<EmptyDB>, NoOpInspector>;

        fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
            Ok(())
        }

        fn execute_transaction_with_commit_condition(
            &mut self,
            _tx: impl alloy_evm::block::ExecutableTx<Self>,
            _f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>) -> CommitChanges,
        ) -> Result<Option<u64>, BlockExecutionError> {
            Ok(Some(0))
        }

        fn finish(
            self,
        ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
            let Self { evm, hook, .. } = self;

            // Call hook with our mock state
            if let Some(mut hook) = hook {
                hook.on_state(StateChangeSource::Transaction(0), &self.state);
            }

            Ok((
                evm,
                BlockExecutionResult {
                    receipts: vec![],
                    requests: Requests::default(),
                    gas_used: 0,
                },
            ))
        }

        fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
            self.hook = hook;
        }

        fn evm(&self) -> &Self::Evm {
            &self.evm
        }

        fn evm_mut(&mut self) -> &mut Self::Evm {
            &mut self.evm
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
    fn test_executor_metrics_hook_metrics_recorded() {
        let snapshotter = setup_test_recorder();
        let metrics = ExecutorMetrics::default();
        let input = RecoveredBlock::<reth_ethereum_primitives::Block>::default();

        let (tx, _rx) = mpsc::channel();
        let expected_output = 42;
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: expected_output });

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
        let _result = metrics
            .execute_metered::<_, EmptyDB>(
                executor,
                input.clone_transactions_recovered().map(Ok::<_, BlockExecutionError>),
                state_hook,
            )
            .unwrap();

        let snapshot = snapshotter.snapshot().into_vec();

        for metric in snapshot {
            let metric_name = metric.0.key().name();
            if metric_name == "sync.execution.accounts_loaded_histogram" ||
                metric_name == "sync.execution.storage_slots_loaded_histogram" ||
                metric_name == "sync.execution.bytecodes_loaded_histogram"
            {
                if let DebugValue::Histogram(vs) = metric.3 {
                    assert!(
                        vs.iter().any(|v| v.into_inner() > 0.0),
                        "metric {metric_name} not recorded"
                    );
                }
            }
        }
    }

    #[test]
    fn test_executor_metrics_hook_called() {
        let metrics = ExecutorMetrics::default();
        let input = RecoveredBlock::<reth_ethereum_primitives::Block>::default();

        let (tx, rx) = mpsc::channel();
        let expected_output = 42;
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: expected_output });

        let state = EvmState::default();

        let executor = MockExecutor::new(state);
        let _result = metrics
            .execute_metered::<_, EmptyDB>(
                executor,
                input.clone_transactions_recovered().map(Ok::<_, BlockExecutionError>),
                state_hook,
            )
            .unwrap();

        let actual_output = rx.try_recv().unwrap();
        assert_eq!(actual_output, expected_output);
    }
}
