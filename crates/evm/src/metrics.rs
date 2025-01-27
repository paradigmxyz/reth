//! Executor metrics.
//!
//! Block processing related to syncing should take care to update the metrics by using either
//! [`ExecutorMetrics::execute_metered`] or [`ExecutorMetrics::metered_one`].
use crate::{execute::Executor, system_calls::OnStateHook};
use alloy_consensus::BlockHeader;
use metrics::{Counter, Gauge, Histogram};
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::Metrics;
use reth_primitives::RecoveredBlock;
use revm_primitives::EvmState;
use std::time::Instant;

/// Wrapper struct that combines metrics and state hook
struct MeteredStateHook {
    metrics: ExecutorMetrics,
    inner_hook: Box<dyn OnStateHook>,
}

impl OnStateHook for MeteredStateHook {
    fn on_state(&mut self, state: &EvmState) {
        // Update the metrics for the number of accounts, storage slots and bytecodes loaded
        let accounts = state.keys().len();
        let storage_slots = state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = state
            .values()
            .filter(|account| !account.info.is_empty_code_hash())
            .collect::<Vec<_>>()
            .len();

        self.metrics.accounts_loaded_histogram.record(accounts as f64);
        self.metrics.storage_slots_loaded_histogram.record(storage_slots as f64);
        self.metrics.bytecodes_loaded_histogram.record(bytecodes as f64);

        // Call the original state hook
        self.inner_hook.on_state(state);
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
    fn metered<F, R, B>(&self, block: &RecoveredBlock<B>, f: F) -> R
    where
        F: FnOnce() -> R,
        B: reth_primitives_traits::Block,
    {
        // Execute the block and record the elapsed time.
        let execute_start = Instant::now();
        let output = f();
        let execution_duration = execute_start.elapsed().as_secs_f64();

        // Update gas metrics.
        self.gas_processed_total.increment(block.header().gas_used());
        self.gas_per_second.set(block.header().gas_used() as f64 / execution_duration);
        self.execution_histogram.record(execution_duration);
        self.execution_duration.set(execution_duration);

        output
    }

    /// Execute the given block using the provided [`Executor`] and update metrics for the
    /// execution.
    ///
    /// Compared to [`Self::metered_one`], this method additionally updates metrics for the number
    /// of accounts, storage slots and bytecodes loaded and updated.
    /// Execute the given block using the provided [`Executor`] and update metrics for the
    /// execution.
    pub fn execute_metered<'a, E, DB, O, Error, B>(
        &self,
        executor: E,
        input: &'a RecoveredBlock<B>,
        state_hook: Box<dyn OnStateHook>,
    ) -> Result<BlockExecutionOutput<O>, Error>
    where
        E: Executor<
            DB,
            Input<'a> = &'a RecoveredBlock<B>,
            Output = BlockExecutionOutput<O>,
            Error = Error,
        >,
        B: reth_primitives_traits::Block,
    {
        // clone here is cheap, all the metrics are Option<Arc<_>>. additionally
        // they are gloally registered so that the data recorded in the hook will
        // be accessible.
        let wrapper = MeteredStateHook { metrics: self.clone(), inner_hook: state_hook };

        // Use metered to execute and track timing/gas metrics
        let output = self.metered(input, || executor.execute_with_state_hook(input, wrapper))?;

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
        self.metered(input, || f(input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use revm::db::BundleState;
    use revm_primitives::{
        Account, AccountInfo, AccountStatus, EvmState, EvmStorage, EvmStorageSlot, B256, U256,
    };
    use std::sync::mpsc;

    /// A mock executor that simulates state changes
    struct MockExecutor {
        state: EvmState,
    }

    impl Executor<()> for MockExecutor {
        type Input<'a>
            = &'a RecoveredBlock<reth_primitives::Block>
        where
            Self: 'a;
        type Output = BlockExecutionOutput<()>;
        type Error = std::convert::Infallible;

        fn execute(self, _input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
            Ok(BlockExecutionOutput {
                state: BundleState::default(),
                receipts: vec![],
                requests: Requests::default(),
                gas_used: 0,
            })
        }
        fn execute_with_state_closure<F>(
            self,
            _input: Self::Input<'_>,
            _state: F,
        ) -> Result<Self::Output, Self::Error>
        where
            F: FnMut(&revm::State<()>),
        {
            Ok(BlockExecutionOutput {
                state: BundleState::default(),
                receipts: vec![],
                requests: Requests::default(),
                gas_used: 0,
            })
        }
        fn execute_with_state_hook<F>(
            self,
            _input: Self::Input<'_>,
            mut hook: F,
        ) -> Result<Self::Output, Self::Error>
        where
            F: OnStateHook + 'static,
        {
            // Call hook with our mock state
            hook.on_state(&self.state);

            Ok(BlockExecutionOutput {
                state: BundleState::default(),
                receipts: vec![],
                requests: Requests::default(),
                gas_used: 0,
            })
        }
    }

    struct ChannelStateHook {
        output: i32,
        sender: mpsc::Sender<i32>,
    }

    impl OnStateHook for ChannelStateHook {
        fn on_state(&mut self, _state: &EvmState) {
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
        let input = RecoveredBlock::default();

        let (tx, _rx) = mpsc::channel();
        let expected_output = 42;
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: expected_output });

        let state = {
            let mut state = EvmState::default();
            let storage =
                EvmStorage::from_iter([(U256::from(1), EvmStorageSlot::new(U256::from(2)))]);
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
                    status: AccountStatus::Loaded,
                },
            );
            state
        };
        let executor = MockExecutor { state };
        let _result = metrics.execute_metered(executor, &input, state_hook).unwrap();

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
        let input = RecoveredBlock::default();

        let (tx, rx) = mpsc::channel();
        let expected_output = 42;
        let state_hook = Box::new(ChannelStateHook { sender: tx, output: expected_output });

        let state = EvmState::default();

        let executor = MockExecutor { state };
        let _result = metrics.execute_metered(executor, &input, state_hook).unwrap();

        let actual_output = rx.try_recv().unwrap();
        assert_eq!(actual_output, expected_output);
    }
}
