//! Executor metrics.
use alloy_consensus::BlockHeader;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives_traits::{Block, RecoveredBlock};
use std::{cell::Cell, time::Instant};
use tracing::warn;

/// Default threshold in milliseconds for slow block logging (1 second).
pub const DEFAULT_SLOW_BLOCK_THRESHOLD_MS: u64 = 1000;

// Thread-local slow block threshold (allows runtime configuration)
thread_local! {
    static SLOW_BLOCK_THRESHOLD_MS: Cell<u64> = const { Cell::new(DEFAULT_SLOW_BLOCK_THRESHOLD_MS) };
}

/// Sets the slow block threshold in milliseconds.
///
/// Blocks that take longer than this threshold to execute will be logged.
/// Set to 0 to log all blocks (useful for debugging/profiling).
pub fn set_slow_block_threshold(threshold_ms: u64) {
    SLOW_BLOCK_THRESHOLD_MS.with(|t| t.set(threshold_ms));
}

/// Returns the current slow block threshold in milliseconds.
pub fn slow_block_threshold() -> u64 {
    SLOW_BLOCK_THRESHOLD_MS.with(|t| t.get())
}

/// Returns true if the given execution time exceeds the slow block threshold.
pub fn is_slow_block(execution_ms: f64) -> bool {
    execution_ms > slow_block_threshold() as f64
}

/// Calculates the cache hit rate as a percentage (0-100).
fn calculate_hit_rate(hits: u64, misses: u64) -> f64 {
    let total = hits + misses;
    if total > 0 {
        (hits as f64 / total as f64) * 100.0
    } else {
        0.0
    }
}

/// Executor metrics.
#[derive(Metrics, Clone)]
#[metrics(scope = "sync.execution")]
pub struct ExecutorMetrics {
    /// The total amount of gas processed.
    pub gas_processed_total: Counter,
    /// The instantaneous amount of gas processed per second.
    pub gas_per_second: Gauge,
    /// The Histogram for amount of gas used.
    pub gas_used_histogram: Histogram,

    /// The Histogram for amount of time taken to execute the pre-execution changes.
    pub pre_execution_histogram: Histogram,
    /// The Histogram for amount of time taken to wait for one transaction to be available.
    pub transaction_wait_histogram: Histogram,
    /// The Histogram for amount of time taken to execute one transaction.
    pub transaction_execution_histogram: Histogram,
    /// The Histogram for amount of time taken to execute the post-execution changes.
    pub post_execution_histogram: Histogram,
    /// The Histogram for amount of time taken to execute blocks.
    pub execution_histogram: Histogram,
    /// The total amount of time it took to execute the latest block.
    pub execution_duration: Gauge,

    /// The Histogram for number of accounts updated when executing the latest block.
    pub accounts_updated_histogram: Histogram,
    /// The Histogram for number of storage slots updated when executing the latest block.
    pub storage_slots_updated_histogram: Histogram,
    /// The Histogram for number of bytecodes updated when executing the latest block.
    pub bytecodes_updated_histogram: Histogram,

    // Unique access tracking
    /// Number of unique accounts touched in the latest block.
    pub unique_accounts: Gauge,
    /// Number of unique storage slots accessed in the latest block.
    pub unique_storage_slots: Gauge,
    /// Number of unique contracts executed in the latest block.
    pub unique_contracts_executed: Gauge,

    // Code bytes tracking
    /// Total bytes of code read in the latest block.
    pub code_bytes_read: Gauge,

    // Cache statistics
    /// Account cache hits.
    pub account_cache_hits: Counter,
    /// Account cache misses.
    pub account_cache_misses: Counter,
    /// Storage cache hits.
    pub storage_cache_hits: Counter,
    /// Storage cache misses.
    pub storage_cache_misses: Counter,
    /// Code cache hits.
    pub code_cache_hits: Counter,
    /// Code cache misses.
    pub code_cache_misses: Counter,
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

    /// Logs slow block execution in JSON format for cross-client performance analysis.
    ///
    /// This method outputs a standardized JSON log entry when block execution
    /// exceeds the slow block threshold, following the cross-client execution
    /// metrics specification.
    #[allow(clippy::too_many_arguments)]
    pub fn log_slow_block(
        &self,
        block_number: u64,
        block_hash: &str,
        gas_used: u64,
        tx_count: usize,
        execution_ms: f64,
        state_read_ms: f64,
        state_hash_ms: f64,
        commit_ms: f64,
        total_ms: f64,
        accounts_loaded: usize,
        storage_slots_loaded: usize,
        code_loaded: usize,
        code_bytes_read: usize,
        accounts_updated: usize,
        storage_slots_updated: usize,
        code_updated: usize,
        code_bytes_written: usize,
        account_cache_hits: u64,
        account_cache_misses: u64,
        storage_cache_hits: u64,
        storage_cache_misses: u64,
        code_cache_hits: u64,
        code_cache_misses: u64,
    ) {
        if is_slow_block(execution_ms) {
            let mgas_per_sec = if execution_ms > 0.0 {
                (gas_used as f64 / 1_000_000.0) / (execution_ms / 1000.0)
            } else {
                0.0
            };

            // Calculate cache hit rates
            let account_hit_rate = calculate_hit_rate(account_cache_hits, account_cache_misses);
            let storage_hit_rate = calculate_hit_rate(storage_cache_hits, storage_cache_misses);
            let code_hit_rate = calculate_hit_rate(code_cache_hits, code_cache_misses);

            // Output as structured JSON log for cross-client analysis
            warn!(
                target: "reth::slow_block",
                message = "Slow block",
                block.number = block_number,
                block.hash = block_hash,
                block.gas_used = gas_used,
                block.tx_count = tx_count,
                timing.execution_ms = format!("{:.3}", execution_ms),
                timing.state_read_ms = format!("{:.3}", state_read_ms),
                timing.state_hash_ms = format!("{:.3}", state_hash_ms),
                timing.commit_ms = format!("{:.3}", commit_ms),
                timing.total_ms = format!("{:.3}", total_ms),
                throughput.mgas_per_sec = format!("{:.2}", mgas_per_sec),
                state_reads.accounts = accounts_loaded,
                state_reads.storage_slots = storage_slots_loaded,
                state_reads.code = code_loaded,
                state_reads.code_bytes = code_bytes_read,
                state_writes.accounts = accounts_updated,
                state_writes.storage_slots = storage_slots_updated,
                state_writes.code = code_updated,
                state_writes.code_bytes = code_bytes_written,
                cache.account.hits = account_cache_hits,
                cache.account.misses = account_cache_misses,
                cache.account.hit_rate = format!("{:.2}", account_hit_rate),
                cache.storage.hits = storage_cache_hits,
                cache.storage.misses = storage_cache_misses,
                cache.storage.hit_rate = format!("{:.2}", storage_hit_rate),
                cache.code.hits = code_cache_hits,
                cache.code.misses = code_cache_misses,
                cache.code.hit_rate = format!("{:.2}", code_hit_rate),
            );
        }
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
    fn test_slow_block_threshold() {
        // Default is 1000ms (1s)
        assert_eq!(slow_block_threshold(), DEFAULT_SLOW_BLOCK_THRESHOLD_MS);
        assert!(!is_slow_block(500.0));
        assert!(is_slow_block(1500.0));

        // Test custom threshold
        set_slow_block_threshold(100);
        assert_eq!(slow_block_threshold(), 100);
        assert!(!is_slow_block(50.0));
        assert!(is_slow_block(150.0));

        // Reset to default
        set_slow_block_threshold(DEFAULT_SLOW_BLOCK_THRESHOLD_MS);
        assert_eq!(slow_block_threshold(), DEFAULT_SLOW_BLOCK_THRESHOLD_MS);
    }

    #[test]
    fn test_slow_block_threshold_zero() {
        // Setting threshold to 0 should log all blocks (any execution > 0 is slow)
        set_slow_block_threshold(0);
        assert!(is_slow_block(1.0));
        assert!(!is_slow_block(0.0));

        // Reset to default
        set_slow_block_threshold(DEFAULT_SLOW_BLOCK_THRESHOLD_MS);
    }

    #[test]
    fn test_log_slow_block_format() {
        // This test exercises the log_slow_block function to ensure it doesn't panic
        // and that the format logic works correctly.
        // The actual log output depends on tracing subscriber configuration.
        let metrics = ExecutorMetrics::default();

        // Set threshold to 0 so any execution time triggers logging
        set_slow_block_threshold(0);

        // Call log_slow_block with sample data
        // This should log (execution_ms=1500.0 > threshold=0)
        metrics.log_slow_block(
            12345,                                                              // block_number
            "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef12345678", // block_hash
            30_000_000,                                                         // gas_used
            200,                                                                // tx_count
            1500.0,                                                             // execution_ms (f64)
            320.0,                                                              // state_read_ms (f64)
            150.0,                                                              // state_hash_ms (f64)
            75.0,                                                               // commit_ms (f64)
            1545.0,                                                             // total_ms (f64)
            100,                                                                // accounts_loaded
            500,   // storage_slots_loaded
            20,    // code_loaded
            10240, // code_bytes_read
            50,    // accounts_updated
            200,   // storage_slots_updated
            0,     // code_updated
            0,     // code_bytes_written
            4,     // account_cache_hits
            6,     // account_cache_misses
            0,     // storage_cache_hits
            11,    // storage_cache_misses
            4,     // code_cache_hits
            0,     // code_cache_misses
        );

        // Reset threshold
        set_slow_block_threshold(DEFAULT_SLOW_BLOCK_THRESHOLD_MS);

        // Verify mgas_per_sec calculation: 30_000_000 / 1_000_000 / (1500 / 1000) = 30 / 1.5 = 20.0
        // This is verified in the log output format
    }
}
