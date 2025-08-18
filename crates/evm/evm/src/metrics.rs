//! Executor metrics.
use alloy_consensus::BlockHeader;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives_traits::{Block, RecoveredBlock};
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
    /// This is a simple helper that tracks execution time and gas usage.
    /// For more complex metrics tracking (like state changes), use the
    /// metered execution functions in the engine/tree module.
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
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::SealedHeader;

    fn create_test_block_with_gas(gas_used: u64) -> RecoveredBlock<Block> {
        let header = Header { gas_used, ..Default::default() };
        RecoveredBlock::new_sealed(
            Block { header: header.clone(), body: Default::default() }.seal_slow(),
            Default::default(),
        )
    }

    #[test]
    fn test_metered_one_updates_metrics() {
        let metrics = ExecutorMetrics::default();
        let block = create_test_block_with_gas(1000);

        // Check initial state
        assert_eq!(metrics.gas_processed_total.get(), 0);

        // Execute with metered_one
        let result = metrics.metered_one(&block, |b| {
            // Simulate some work
            std::thread::sleep(std::time::Duration::from_millis(10));
            b.header().gas_used()
        });

        // Verify result
        assert_eq!(result, 1000);

        // Verify metrics were updated
        assert_eq!(metrics.gas_processed_total.get(), 1000);
        assert!(metrics.gas_per_second.get() > 0.0);
        assert!(metrics.execution_duration.get() > 0.0);
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
        assert_eq!(metrics.gas_processed_total.get(), 500);
        assert!(metrics.execution_duration.get() >= 0.01); // At least 10ms
    }
}
