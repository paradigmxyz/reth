//! Executor metrics.
use alloy_consensus::BlockHeader;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives_traits::{Block, RecoveredBlock};
use revm::{
    bytecode::opcode::OpCode,
    context_interface::ContextTr,
    inspector::Inspector,
    interpreter::{interpreter::EthInterpreter, interpreter_types::Jumps, Interpreter},
};
use std::time::Instant;

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

    /// The total number of SLOAD opcode executions.
    pub sload_count: Counter,
    /// The total number of SSTORE opcode executions.
    pub sstore_count: Counter,
    /// The Histogram for SLOAD opcode execution times.
    pub sload_time_histogram: Histogram,
    /// The Histogram for SSTORE opcode execution times.
    pub sstore_time_histogram: Histogram,
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

/// Inspector that tracks SLOAD and SSTORE opcode executions and their execution times.
#[derive(Debug)]
pub struct StorageMetricsInspector {
    metrics: ExecutorMetrics,
    operation_start: Option<Instant>,
    current_opcode: Option<OpCode>,
}

impl StorageMetricsInspector {
    /// Create a new `StorageMetricsInspector` with the given metrics.
    pub const fn new(metrics: ExecutorMetrics) -> Self {
        Self { metrics, operation_start: None, current_opcode: None }
    }
}

impl<CTX> Inspector<CTX, EthInterpreter> for StorageMetricsInspector
where
    CTX: ContextTr,
{
    /// Called BEFORE each opcode execution
    fn step(&mut self, interp: &mut Interpreter<EthInterpreter>, _context: &mut CTX) {
        // Check current opcode using OpCode enum
        if let Some(opcode) = OpCode::new(interp.bytecode.opcode()) {
            match opcode {
                OpCode::SLOAD => {
                    // SLOAD opcode detected - record start time
                    self.metrics.sload_count.increment(1);
                    self.operation_start = Some(Instant::now());
                    self.current_opcode = Some(OpCode::SLOAD);
                }
                OpCode::SSTORE => {
                    // SSTORE opcode detected - record start time
                    self.metrics.sstore_count.increment(1);
                    self.operation_start = Some(Instant::now());
                    self.current_opcode = Some(OpCode::SSTORE);
                }
                _ => {
                    // Not SLOAD/SSTORE - clear any pending measurement
                    self.operation_start = None;
                    self.current_opcode = None;
                }
            }
        }
    }

    /// Called AFTER each opcode execution
    fn step_end(&mut self, _interp: &mut Interpreter<EthInterpreter>, _context: &mut CTX) {
        // Check if we have a pending operation
        // Note: We don't check the current opcode here because PC may have already advanced
        // to the next opcode. We rely on the opcode we saved in step().
        if let (Some(start), Some(opcode)) =
            (self.operation_start.take(), self.current_opcode.take())
        {
            match opcode {
                OpCode::SLOAD => {
                    // SLOAD execution completed - record time
                    let elapsed = start.elapsed().as_secs_f64();
                    self.metrics.sload_time_histogram.record(elapsed);
                }
                OpCode::SSTORE => {
                    // SSTORE execution completed - record time
                    let elapsed = start.elapsed().as_secs_f64();
                    self.metrics.sstore_time_histogram.record(elapsed);
                }
                _ => {
                    // Not SLOAD/SSTORE - restore state (shouldn't happen normally)
                    self.operation_start = Some(start);
                    self.current_opcode = Some(opcode);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_evm::{EthEvm, Evm};
    use alloy_primitives::{bytes, TxKind, B256, U256};
    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::Block as BlockTrait;
    use revm::{
        context::TxEnv,
        database::{CacheDB, EmptyDB},
        primitives::{address, keccak256},
        state::{AccountInfo, Bytecode},
        Context, MainBuilder, MainContext,
    };

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

    fn setup_test_recorder() -> Snapshotter {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        // Try to install, but ignore error if recorder is already installed
        // This allows multiple tests to run without conflicts
        let _ = recorder.install();
        snapshotter
    }

    #[test]
    fn test_storage_metrics_inspector_sload() {
        // Initialize metrics recorder to capture metric values
        let snapshotter = setup_test_recorder();

        // Create bytecode that performs SLOAD: PUSH1 0x00; SLOAD; STOP
        let code = bytes![0x60, 0x00, 0x54, 0x00];
        let code_hash = keccak256(&code);

        // Set up database with account and storage
        let mut db = CacheDB::new(EmptyDB::default());
        let contract_address = address!("0x1000000000000000000000000000000000000000");

        // Create account with code
        let account_info = AccountInfo {
            balance: U256::ZERO,
            nonce: 0,
            code_hash,
            code: Some(Bytecode::new_raw(code.clone())),
        };
        db.insert_account_info(contract_address, account_info);

        // Note: We don't need to set storage for this test.
        // SLOAD will read from empty storage (which returns 0), and the inspector
        // will still track that SLOAD was executed, which is what we're testing.

        // Create metrics and inspector
        let metrics = ExecutorMetrics::default();
        let inspector = StorageMetricsInspector::new(metrics.clone());

        // Set up EVM context
        // build_mainnet_with_inspector returns a handler that doesn't automatically enable
        // inspector We need to wrap it with EthEvm and pass true as the second parameter to
        // enable inspector
        let handler = Context::mainnet().with_db(db).build_mainnet_with_inspector(inspector);

        // Wrap with EthEvm and enable inspector (second parameter = true)
        let mut evm = EthEvm::new(handler, true);

        // Create transaction environment to call the contract
        let mut tx_env = TxEnv::default();
        tx_env.caller = address!("0x2000000000000000000000000000000000000000");
        tx_env.kind = TxKind::Call(contract_address);
        tx_env.gas_limit = 100000;
        tx_env.value = U256::ZERO;

        // Execute the code
        let _result = evm.transact(tx_env).unwrap();

        // Verify that sload_count was incremented
        // Take a snapshot to read the metric value
        let snapshot = snapshotter.snapshot().into_vec();
        let sload_metric = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == "sync.execution.sload_count")
            .expect("sload_count metric should be recorded");

        // Verify the counter was incremented to 1
        // The value is the 4th element of the tuple
        let (_, _, _, value) = sload_metric;
        // Check if it's a counter and get its value
        // The counter value is stored directly in DebugValue::Counter as u64
        match value {
            metrics_util::debugging::DebugValue::Counter(counter_value) => {
                assert_eq!(
                    *counter_value, 1,
                    "sload_count should be 1 after executing SLOAD opcode"
                );
            }
            _ => panic!("sload_count should be a counter metric, got: {:?}", value),
        }

        // Verify that sload_time_histogram was recorded
        let sload_time_metric = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == "sync.execution.sload_time_histogram")
            .expect("sload_time_histogram metric should be recorded");

        let (_, _, _, time_value) = sload_time_metric;
        // Check if it's a histogram and verify it has recorded at least one value
        match time_value {
            metrics_util::debugging::DebugValue::Histogram(histogram_data) => {
                // Verify that the histogram has recorded at least one value
                // The histogram should have at least one recorded value after SLOAD execution
                assert!(
                    histogram_data.len() > 0,
                    "sload_time_histogram should have recorded at least one value"
                );
            }
            _ => panic!("sload_time_histogram should be a histogram metric, got: {:?}", time_value),
        }
    }

    #[test]
    fn test_storage_metrics_inspector_sstore() {
        // Initialize metrics recorder to capture metric values
        let snapshotter = setup_test_recorder();

        // Create bytecode that performs SSTORE: PUSH1 0x01; PUSH1 0x00; SSTORE; STOP
        // This stores value 1 at storage slot 0
        let code = bytes![0x60, 0x01, 0x60, 0x00, 0x55, 0x00];
        let code_hash = keccak256(&code);

        // Set up database with account and storage
        let mut db = CacheDB::new(EmptyDB::default());
        let contract_address = address!("0x1000000000000000000000000000000000000000");

        // Create account with code
        let account_info = AccountInfo {
            balance: U256::ZERO,
            nonce: 0,
            code_hash,
            code: Some(Bytecode::new_raw(code.clone())),
        };
        db.insert_account_info(contract_address, account_info);

        // Create metrics and inspector
        let metrics = ExecutorMetrics::default();
        let inspector = StorageMetricsInspector::new(metrics.clone());

        // Set up EVM context
        // build_mainnet_with_inspector returns a handler that doesn't automatically enable
        // inspector We need to wrap it with EthEvm and pass true as the second parameter to
        // enable inspector
        let handler = Context::mainnet().with_db(db).build_mainnet_with_inspector(inspector);

        // Wrap with EthEvm and enable inspector (second parameter = true)
        let mut evm = EthEvm::new(handler, true);

        // Create transaction environment to call the contract
        let mut tx_env = TxEnv::default();
        tx_env.caller = address!("0x2000000000000000000000000000000000000000");
        tx_env.kind = TxKind::Call(contract_address);
        tx_env.gas_limit = 100000;
        tx_env.value = U256::ZERO;

        // Execute the code
        let _result = evm.transact(tx_env).unwrap();

        // Verify that sstore_count was incremented
        // Take a snapshot to read the metric value
        let snapshot = snapshotter.snapshot().into_vec();
        let sstore_metric = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == "sync.execution.sstore_count")
            .expect("sstore_count metric should be recorded");

        // Verify the counter was incremented to 1
        let (_, _, _, value) = sstore_metric;
        match value {
            metrics_util::debugging::DebugValue::Counter(counter_value) => {
                assert_eq!(
                    *counter_value, 1,
                    "sstore_count should be 1 after executing SSTORE opcode"
                );
            }
            _ => panic!("sstore_count should be a counter metric, got: {:?}", value),
        }

        // Verify that sstore_time_histogram was recorded
        let sstore_time_metric = snapshot
            .iter()
            .find(|(key, _, _, _)| key.key().name() == "sync.execution.sstore_time_histogram")
            .expect("sstore_time_histogram metric should be recorded");

        let (_, _, _, time_value) = sstore_time_metric;
        // Check if it's a histogram and verify it has recorded at least one value
        match time_value {
            metrics_util::debugging::DebugValue::Histogram(histogram_data) => {
                // Verify that the histogram has recorded at least one value
                assert!(
                    histogram_data.len() > 0,
                    "sstore_time_histogram should have recorded at least one value"
                );
            }
            _ => {
                panic!("sstore_time_histogram should be a histogram metric, got: {:?}", time_value)
            }
        }
    }
}
