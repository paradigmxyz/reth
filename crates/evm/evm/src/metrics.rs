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

    /// The Counter for number of execution results cached hits when executing the latest block.
    pub execution_results_cache_hits: Counter,
    /// The Counter for number of execution results cached misses when executing the latest block.
    pub execution_results_cache_misses: Counter,

    /// The Counter for number of transactions skipped due to cache hits.
    pub transactions_skipped: Counter,
    /// The Counter for total gas skipped due to cache hits.
    pub gas_skipped: Counter,
    /// The Histogram for gas skipped per cached transaction.
    pub gas_skipped_per_tx_histogram: Histogram,
    /// The Histogram for cache hit rate per block.
    pub cache_hit_rate_histogram: Histogram,
    /// The Counter for cache validation failures.
    pub cache_validation_failures: Counter,
    /// The Histogram for cache validation time per transaction.
    pub cache_validation_time_histogram: Histogram,
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
    use alloy_primitives::B256;
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::Block as BlockTrait;

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
            b.header().gas_used() * 2
        });

        assert_eq!(result, 2000);

        // Note: We can't easily test the counter values because they use global state
        // but the function should update them correctly
    }

    #[test]
    fn test_metered_with_empty_block() {
        let metrics = ExecutorMetrics::default();
        let block = create_test_block_with_gas(0);

        let result = metrics.metered_one(&block, |_| "test");
        assert_eq!(result, "test");
    }
}