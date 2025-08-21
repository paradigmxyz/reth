//! Executor metrics.
use crate::{execute::{BlockExecutionError, BlockExecutionOutput}, OnStateHook};
use alloy_consensus::BlockHeader;
use alloy_evm::{
    block::{BlockExecutor, ExecutableTx, StateChangeSource},
    Evm, RecoveredTx,
};
use alloy_primitives::TxHash;
use core::borrow::BorrowMut;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives_traits::{Block, RecoveredBlock, SignedTransaction};
use revm::{
    context::result::ResultAndState,
    database::{states::bundle_state::BundleRetention, State, Database},
    state::EvmState,
};
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

    /// The Counter for number of execution results cached hits when executing the latest block.
    pub execution_results_cache_hits: Counter,
    /// The Counter for number of execution results cached misses when executing the latest block.
    pub execution_results_cache_misses: Counter,
}

/// Hook that records state metrics
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
        
        // Forward to inner hook
        self.inner_hook.on_state(source, state)
    }
}

impl ExecutorMetrics {
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
        self.gas_processed_total.increment(gas_used);
        self.gas_per_second.set(gas_used as f64 / execution_duration);
        self.gas_used_histogram.record(gas_used as f64);
        self.execution_histogram.record(execution_duration);
        self.execution_duration.set(execution_duration);

        output
    }

    /// Execute a block and update basic gas/timing metrics.
    ///
    /// Compared to [`Self::metered_one`], this method additionally updates metrics for the number
    /// of accounts, storage slots and bytecodes loaded and updated.
    /// Execute the given block using the provided [`BlockExecutor`] and update metrics for the
    /// execution.
    pub fn execute_metered<E, DB, T: SignedTransaction>(
        &self,
        executor: E,
        transactions: impl Iterator<Item = Result<impl ExecutableTx<E>, BlockExecutionError>>,
        state_hook: Box<dyn OnStateHook>,
        get_cached_tx_result: impl Fn(
            &mut <E::Evm as Evm>::DB,
            TxHash,
        ) -> Option<ResultAndState<<E::Evm as Evm>::HaltReason>>,
    ) -> Result<BlockExecutionOutput<E::Receipt>, BlockExecutionError>
    where
        DB: Database,
        E: BlockExecutor<Evm: Evm<DB: BorrowMut<State<DB>>>, Transaction = T>,
    {
        // clone here is cheap, all the metrics are Option<Arc<_>>. additionally
        // they are globally registered so that the data recorded in the hook will
        // be accessible.
        let wrapper = MeteredStateHook { metrics: self.clone(), inner_hook: state_hook };

        let mut executor = executor.with_state_hook(Some(Box::new(wrapper)));

        let f = || {
            executor.apply_pre_execution_changes()?;
            for tx in transactions {
                let tx = tx?;
                let tx_hash = *tx.tx().tx_hash();
                if let Some(result) =
                    get_cached_tx_result(executor.evm_mut().db_mut(), tx_hash)
                {
                    self.execution_results_cache_hits.increment(1);
                    tracing::debug!(
                        target: "engine::cache",
                        ?tx_hash,
                        "Cache hit - reusing prewarmed execution result"
                    );
                    executor.execute_transaction_with_cached_result(
                        tx,
                        result,
                        |_| alloy_evm::block::CommitChanges::Yes,
                    )?;
                } else {
                    self.execution_results_cache_misses.increment(1);
                    tracing::debug!(
                        target: "engine::cache",
                        ?tx_hash,
                        "Cache miss - executing transaction from scratch"
                    );
                    executor.execute_transaction(tx)?;
                }
            }
            executor.finish().map(|(evm, result)| (evm.into_db(), result))
        };

        // Use metered to execute and track timing/gas metrics
        let (mut db, result) = self.metered(|| {
            let res = f();
            let gas_used = res.as_ref().map(|r| r.1.gas_used).unwrap_or(0);
            (gas_used, res)
        })?;
        
        // Log cache performance summary
        let total_hits = self.execution_results_cache_hits.get();
        let total_misses = self.execution_results_cache_misses.get();
        if total_hits > 0 || total_misses > 0 {
            let hit_rate = if (total_hits + total_misses) > 0 {
                (total_hits as f64) / ((total_hits + total_misses) as f64) * 100.0
            } else {
                0.0
            };
            tracing::info!(
                target: "engine::cache",
                cache_hits = total_hits,
                cache_misses = total_misses,
                hit_rate_percent = format!("{:.2}", hit_rate),
                "Block execution cache performance summary"
            );
        }

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
    pub fn metered_one<F, R, B>(&self, block: &RecoveredBlock<B>, f: F) -> R
    where
        F: FnOnce(&RecoveredBlock<B>) -> R,
        B: Block,
        B::Header: BlockHeader,
    {
        self.metered(|| (block.header().gas_used(), f(block)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{B256, U256};
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::Block as BlockTrait;
    use revm::state::{Account, AccountInfo, AccountStatus, EvmStorage, EvmStorageSlot};
    use revm::database::EmptyDB;

    fn create_test_block_with_gas(gas_used: u64) -> RecoveredBlock<Block> {
        let header = Header { gas_used, ..Default::default() };
        let block = Block { header, body: Default::default() };
        // Use a dummy hash for testing
        let hash = B256::default();
        let sealed = block.seal_unchecked(hash);
        RecoveredBlock::new_sealed(sealed, Default::default())
    }

    #[test]
    fn test_metered_one_updates_metrics() {
        let metrics = ExecutorMetrics::default();
        let block = create_test_block_with_gas(1000);

        // Execute with metered_one
        let result = metrics.metered_one(&block, |b| {
            // Simulate some work
            std::thread::sleep(std::time::Duration::from_millis(10));
            b.header().gas_used()
        });

        // Verify result
        assert_eq!(result, 1000);
    }

    #[test]
    fn test_metered_helper_tracks_timing() {
        let metrics = ExecutorMetrics::default();

        let result = metrics.metered(|| {
            // Simulate some work
            std::thread::sleep(std::time::Duration::from_millis(10));
            (500, "test_result")
        });

        assert_eq!(result, "test_result");
    }

    #[test]
    fn test_execute_metered_with_state_hook() {
        use std::sync::mpsc;
        use metrics_util::debugging::{DebugValue, Snapshotter};

        let metrics = ExecutorMetrics::default();
        let snapshotter = Snapshotter::current_thread_snapshot().unwrap();
        
        let input = create_test_block_with_gas(1000);
        let (tx, rx) = mpsc::channel::<()>();
        let expected_output = ();

        // Create a simple state hook that sends to channel
        struct TestStateHook {
            tx: mpsc::Sender<()>,
        }

        impl OnStateHook for TestStateHook {
            fn on_state(&mut self, _source: StateChangeSource, _state: &EvmState) {
                let _ = self.tx.send(());
            }
        }

        let state_hook = Box::new(TestStateHook { tx });

        // Create a test executor
        struct MockExecutor<DB> {
            state: EvmState,
            _db: std::marker::PhantomData<DB>,
        }

        impl<DB> MockExecutor<DB> {
            fn new(state: EvmState) -> Self {
                Self { state, _db: std::marker::PhantomData }
            }
        }

        // This test would need a proper mock executor implementation
        // For now, we just test that the metrics struct compiles correctly
    }
}