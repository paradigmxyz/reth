use reth_interfaces::executor::Error as ExecutionError;
use reth_primitives::{Address, Block, U256};
use reth_provider::{post_state::PostState, BlockExecutor, StateProvider};

/// Test executor with mocked result.
pub struct TestExecutor(pub Option<PostState>);

impl<SP: StateProvider> BlockExecutor<SP> for TestExecutor {
    fn execute(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<PostState, ExecutionError> {
        self.0.clone().ok_or(ExecutionError::VerificationFailed)
    }

    fn execute_and_verify_receipt(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<PostState, ExecutionError> {
        self.0.clone().ok_or(ExecutionError::VerificationFailed)
    }
}
