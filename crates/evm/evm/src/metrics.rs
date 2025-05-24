//! Executor metrics.
//!
//! Block processing related to syncing should take care to update the metrics by using
//! [`ExecutorMetrics::metered_one`].
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use alloy_consensus::BlockHeader;
use reth_primitives_traits::RecoveredBlock;
use std::time::Instant;

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
    /// Updates metrics while executing the given function.
    pub fn metered<F, R, B>(&self, block: &RecoveredBlock<B>, f: F) -> R 
    where 
        F: FnOnce() -> R, 
        B: reth_primitives_traits::Block, 
    { 
        let now = Instant::now(); 
        
        let result = f(); 
        
        let elapsed = now.elapsed(); 
        self.execution_histogram.record(elapsed.as_secs_f64()); 
        self.execution_duration.set(elapsed.as_secs_f64()); 
        
        let gas = block.header().gas_used() as u64; 
        self.gas_processed_total.increment(gas); 
        self.gas_per_second.set(gas as f64 / elapsed.as_secs_f64()); 
        self.gas_used_histogram.record(gas as f64); 
        
        result 
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

/// Wrapper struct that combines metrics and state hook
pub struct MeteredStateHook {
    /// Metrics for the executor
    pub metrics: ExecutorMetrics,
    /// The inner state hook to delegate to
    pub inner_hook: Box<dyn alloy_evm::block::OnStateHook>,
}

impl std::fmt::Debug for MeteredStateHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeteredStateHook")
            .field("metrics", &self.metrics)
            .field("inner_hook", &"<dyn OnStateHook>")
            .finish()
    }
}

impl alloy_evm::block::OnStateHook for MeteredStateHook {
    fn on_state(&mut self, source: alloy_evm::block::StateChangeSource, state: &revm::state::EvmState) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::Requests;
    use alloy_evm::{block::{CommitChanges, StateChangeSource}, EthEvm};
    use alloy_primitives::{B256, U256};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use reth_ethereum_primitives::{Receipt, TransactionSigned};
    use reth_execution_types::BlockExecutionResult;
    use revm::{
        context::result::ExecutionResult,
        database::State,
        database_interface::EmptyDB,
        inspector::NoOpInspector,
        state::{Account, AccountInfo, AccountStatus, EvmStorage, EvmStorageSlot, EvmState},
        Context, MainBuilder, MainContext,
    };
    use std::sync::mpsc;

    /// A mock executor that simulates state changes
    struct MockExecutor {
        state: EvmState,
        hook: Option<Box<dyn alloy_evm::block::OnStateHook>>,
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

    impl alloy_evm::block::BlockExecutor for MockExecutor {
        type Transaction = TransactionSigned;
        type Receipt = Receipt;
        type Evm = EthEvm<State<EmptyDB>, NoOpInspector>;

        fn apply_pre_execution_changes(&mut self) -> Result<(), reth_execution_errors::BlockExecutionError> {
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
        ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), reth_execution_errors::BlockExecutionError> {
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

        fn set_state_hook(&mut self, hook: Option<Box<dyn alloy_evm::block::OnStateHook>>) {
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

    impl alloy_evm::block::OnStateHook for ChannelStateHook {
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
}
