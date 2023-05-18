use crate::{post_state::PostState, BlockExecutor, ExecutorFactory, StateProvider};
use parking_lot::Mutex;
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, ChainSpec, U256};
use std::sync::Arc;
/// Test executor with mocked result.
pub struct TestExecutor(pub Option<PostState>);

impl<SP: StateProvider> BlockExecutor<SP> for TestExecutor {
    fn execute(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError> {
        self.0.clone().ok_or(BlockExecutionError::UnavailableForTest)
    }

    fn execute_and_verify_receipt(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError> {
        self.0.clone().ok_or(BlockExecutionError::UnavailableForTest)
    }
}

/// Executor factory with pre-set execution results.
#[derive(Clone, Debug)]
pub struct TestExecutorFactory {
    exec_results: Arc<Mutex<Vec<PostState>>>,
    chain_spec: Arc<ChainSpec>,
}

impl TestExecutorFactory {
    /// Create new instance of test factory.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { exec_results: Arc::new(Mutex::new(Vec::new())), chain_spec }
    }

    /// Extend the mocked execution results
    pub fn extend(&self, results: Vec<PostState>) {
        self.exec_results.lock().extend(results.into_iter());
    }
}

impl ExecutorFactory for TestExecutorFactory {
    type Executor<T: StateProvider> = TestExecutor;

    fn with_sp<SP: StateProvider>(&self, _sp: SP) -> Self::Executor<SP> {
        let exec_res = self.exec_results.lock().pop();
        TestExecutor(exec_res)
    }

    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
