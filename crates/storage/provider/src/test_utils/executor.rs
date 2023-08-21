use crate::{
    change::BundleState, BlockExecutor, BlockExecutorStats, ExecutorFactory, StateProvider,
};
use parking_lot::Mutex;
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, ChainSpec, U256};
use std::sync::Arc;
/// Test executor with mocked result.
pub struct TestExecutor(pub Option<BundleState>);

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

    fn take_output_state(&mut self) -> BundleState {
        self.0.clone().unwrap_or_default()
    }

    fn stats(&self) -> BlockExecutorStats {
        BlockExecutorStats::default()
    }
}

/// Executor factory with pre-set execution results.
#[derive(Clone, Debug)]
pub struct TestExecutorFactory {
    exec_results: Arc<Mutex<Vec<BundleState>>>,
    chain_spec: Arc<ChainSpec>,
}

impl TestExecutorFactory {
    /// Create new instance of test factory.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { exec_results: Arc::new(Mutex::new(Vec::new())), chain_spec }
    }

    /// Extend the mocked execution results
    pub fn extend(&self, results: Vec<BundleState>) {
        self.exec_results.lock().extend(results);
    }
}

impl ExecutorFactory for TestExecutorFactory {
    fn with_sp<'a, SP: StateProvider + 'a>(&'a self, _sp: SP) -> Box<dyn BlockExecutor + 'a> {
        let exec_res = self.exec_results.lock().pop();
        Box::new(TestExecutor(exec_res))
    }

    fn with_sp_and_bundle<'a, SP: StateProvider + 'a>(
        &'a self,
        _sp: SP,
        mut bundle: BundleState,
    ) -> Box<dyn BlockExecutor + 'a> {
        let exec_res = self.exec_results.lock().pop();
        let bundle = exec_res.map(|res| {
            bundle.extend(res);
            bundle
        });

        Box::new(TestExecutor(bundle))
    }

    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
