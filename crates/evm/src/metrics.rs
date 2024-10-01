//! Executor metrics.
//!
//! Block processing related to syncing should take care to update the metrics by using e.g.
//! [`ExecutorMetrics::metered`].
use std::time::Instant;

use metrics::{Counter, Gauge, Histogram};
use reth_execution_types::BlockExecutionInput;
use reth_metrics::Metrics;
use reth_primitives::BlockWithSenders;

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
}

impl ExecutorMetrics {
    /// Execute the given block and update metrics for the execution.
    pub fn metered<F, R>(&self, input: BlockExecutionInput<'_, BlockWithSenders>, f: F) -> R
    where
        F: FnOnce(BlockExecutionInput<'_, BlockWithSenders>) -> R,
    {
        let gas_used = input.block.gas_used;

        // Execute the block and record the elapsed time.
        let execute_start = Instant::now();
        let output = f(input);
        let execution_duration = execute_start.elapsed().as_secs_f64();

        // Update gas metrics.
        self.gas_processed_total.increment(gas_used);
        self.gas_per_second.set(gas_used as f64 / execution_duration);
        self.execution_histogram.record(execution_duration);
        self.execution_duration.set(execution_duration);

        output
    }
}
