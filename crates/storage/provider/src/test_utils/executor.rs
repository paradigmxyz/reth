use crate::{
    bundle_state::BundleStateWithReceipts, BlockExecutor, BlockExecutorStats, ExecutorFactory,
    PrunableBlockExecutor, StateProvider,
};
use parking_lot::Mutex;
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, BlockNumber, ChainSpec, PruneModes, Receipt, U256};
use std::sync::Arc;
/// Test executor with mocked result.
#[derive(Debug)]
pub struct TestExecutor(pub Option<BundleStateWithReceipts>);

impl BlockExecutor for TestExecutor {
    fn execute(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        if self.0.is_none() {
            return Err(BlockExecutionError::UnavailableForTest)
        }
        Ok(())
    }

    fn execute_and_verify_receipt(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        if self.0.is_none() {
            return Err(BlockExecutionError::UnavailableForTest)
        }
        Ok(())
    }

    fn execute_transactions(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        Err(BlockExecutionError::UnavailableForTest)
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        self.0.clone().unwrap_or_default()
    }

    fn stats(&self) -> BlockExecutorStats {
        BlockExecutorStats::default()
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

impl PrunableBlockExecutor for TestExecutor {
    fn set_tip(&mut self, _tip: BlockNumber) {}

    fn set_prune_modes(&mut self, _prune_modes: PruneModes) {}
}

/// Executor factory with pre-set execution results.
#[derive(Clone, Debug)]
pub struct TestExecutorFactory {
    exec_results: Arc<Mutex<Vec<BundleStateWithReceipts>>>,
    chain_spec: Arc<ChainSpec>,
}

impl TestExecutorFactory {
    /// Create new instance of test factory.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { exec_results: Arc::new(Mutex::new(Vec::new())), chain_spec }
    }

    /// Extend the mocked execution results
    pub fn extend(&self, results: Vec<BundleStateWithReceipts>) {
        self.exec_results.lock().extend(results);
    }
}

impl ExecutorFactory for TestExecutorFactory {
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        _sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a> {
        let exec_res = self.exec_results.lock().pop();
        Box::new(TestExecutor(exec_res))
    }

    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
