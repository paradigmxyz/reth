//! Executor metrics.
//!
//! Block processing related to syncing should take care to update the metrics by using e.g.
//! [`ExecutorMetrics::metered`].
use std::time::Instant;

use metrics::{Counter, Gauge, Histogram};
use reth_execution_types::{BlockExecutionInput, BlockExecutionOutput};
use reth_metrics::Metrics;
use reth_primitives::BlockWithSenders;
use revm::CacheState;

use crate::execute::Executor;

/// Executor metrics.
// TODO(onbjerg): add sload/sstore, acc load/acc change, bytecode metrics
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
    /// The total number of accounts loaded when executing the latest block.
    pub accounts_loaded_total: Counter,
    /// The total number of storage slots loaded when executing the latest block.
    pub storage_slots_loaded_total: Counter,
    /// The total number of bytecodes loaded when executing the latest block.
    pub bytecodes_loaded_total: Counter,
    /// The total number of accounts updated when executing the latest block.
    pub accounts_updated_total: Counter,
    /// The total number of storage slots updated when executing the latest block.
    pub storage_slots_updated_total: Counter,
    /// The total number of bytecodes updated when executing the latest block.
    pub bytecodes_updated_total: Counter,
}

impl ExecutorMetrics {
    fn metered<F, R>(&self, block: &BlockWithSenders, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // Execute the block and record the elapsed time.
        let execute_start = Instant::now();
        let output = f();
        let execution_duration = execute_start.elapsed().as_secs_f64();

        // Update gas metrics.
        self.gas_processed_total.increment(block.gas_used);
        self.gas_per_second.set(block.gas_used as f64 / execution_duration);
        self.execution_histogram.record(execution_duration);
        self.execution_duration.set(execution_duration);

        output
    }

    /// Execute the given block using the provided [`Executor`] and update metrics for the
    /// execution.
    ///
    /// Compared to [`Self::metered_one`], this method additionally updates metrics for the number
    /// of accounts, storage slots and bytecodes loaded and updated.
    pub fn execute_metered_with_output<'a, DB, O, E>(
        &self,
        executor: impl Executor<
            DB,
            Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
            Output = BlockExecutionOutput<O>,
            Error = E,
        >,
        input: BlockExecutionInput<'a, BlockWithSenders>,
    ) -> Result<BlockExecutionOutput<O>, E> {
        let output = self.metered(input.block, || {
            executor.execute_with_state_closure(input, |state| {
                // Help LSP to resolve the type correctly
                let cache_state: &CacheState = &state.cache;

                // Update the metrics for the number of accounts, storage slots and bytecodes
                // loaded
                let accounts = cache_state.accounts.len();
                let storage_slots = cache_state
                    .accounts
                    .values()
                    .filter_map(|account| {
                        account.account.as_ref().map(|account| account.storage.len())
                    })
                    .sum::<usize>();
                let bytecodes = cache_state.contracts.len();

                self.accounts_loaded_total.increment(accounts as u64);
                self.storage_slots_loaded_total.increment(storage_slots as u64);
                self.bytecodes_loaded_total.increment(bytecodes as u64);
            })
        })?;

        // Update the metrics for the number of accounts, storage slots and bytecodes updated
        let accounts = output.state.state.len();
        let storage_slots =
            output.state.state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = output.state.contracts.len();

        self.accounts_updated_total.increment(accounts as u64);
        self.storage_slots_updated_total.increment(storage_slots as u64);
        self.bytecodes_updated_total.increment(bytecodes as u64);

        Ok(output)
    }

    /// Execute the given block and update metrics for the execution.
    pub fn metered_one<F, R>(&self, input: BlockExecutionInput<'_, BlockWithSenders>, f: F) -> R
    where
        F: FnOnce(BlockExecutionInput<'_, BlockWithSenders>) -> R,
    {
        self.metered(input.block, || f(input))
    }
}
