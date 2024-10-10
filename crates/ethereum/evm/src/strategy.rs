//! Ethereum block execution strategy,

use reth_evm::{
    execute::{BlockExecutionError, BlockExecutionStrategy, BlockExecutionStrategyFactory},
    system_calls::OnStateHook,
};
use reth_primitives::Request;
use reth_revm::{db::BundleState, State};

/// Factory for [`EthExecutionStrategy`].
#[derive(Clone, Debug)]
pub struct EthExecutionStrategyFactory {}

impl BlockExecutionStrategyFactory for EthExecutionStrategyFactory {
    type Strategy<
        DB: reth_revm::Database<Error: Into<reth_evm::execute::ProviderError> + core::fmt::Display>,
    > = EthExecutionStrategy<DB>;

    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: reth_revm::Database<Error: Into<reth_evm::execute::ProviderError> + core::fmt::Display>,
    {
        let state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        EthExecutionStrategy::new(state)
    }
}

/// Block execution strategy for Ethereum.
#[allow(missing_debug_implementations)]
pub struct EthExecutionStrategy<DB> {
    state: State<DB>,
}

impl<DB> EthExecutionStrategy<DB> {
    /// Creates a new [`EthExecutionStrategy`]
    pub const fn new(state: State<DB>) -> Self {
        Self { state }
    }
}

impl<DB> BlockExecutionStrategy<DB> for EthExecutionStrategy<DB> {
    type Error = BlockExecutionError;

    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
        todo!()
    }

    fn execute_transactions(
        &mut self,
        _block: &reth_primitives::BlockWithSenders,
    ) -> Result<(Vec<reth_primitives::Receipt>, u64), Self::Error> {
        todo!()
    }

    fn apply_post_execution_changes(&mut self) -> Result<Vec<Request>, Self::Error> {
        todo!()
    }

    fn state_ref(&self) -> &State<DB> {
        &self.state
    }

    fn with_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {
        todo!()
    }

    fn finish(&self) -> BundleState {
        todo!()
    }
}
