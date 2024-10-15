//! Traits for execution.

// Re-export execution types
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
pub use reth_execution_types::{BlockExecutionInput, BlockExecutionOutput, ExecutionOutcome};
pub use reth_storage_errors::provider::ProviderError;

use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::BlockNumber;
use core::{cell::RefCell, fmt::Display, marker::PhantomData};
use reth_primitives::{BlockWithSenders, Receipt, Request};
use reth_prune_types::PruneModes;
use revm::{db::BundleState, State};
use revm_primitives::db::Database;

use crate::system_calls::OnStateHook;

/// A general purpose executor trait that executes an input (e.g. block) and produces an output
/// (e.g. state changes and receipts).
///
/// This executor does not validate the output, see [`BatchExecutor`] for that.
pub trait Executor<DB> {
    /// The input type for the executor.
    type Input<'a>;
    /// The output type for the executor.
    type Output;
    /// The error type returned by the executor.
    type Error;

    /// Consumes the type and executes the block.
    ///
    /// # Note
    /// Execution happens without any validation of the output. To validate the output, use the
    /// [`BatchExecutor`].
    ///
    /// # Returns
    /// The output of the block execution.
    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error>;

    /// Executes the EVM with the given input and accepts a state closure that is invoked with
    /// the EVM state after execution.
    fn execute_with_state_closure<F>(
        self,
        input: Self::Input<'_>,
        state: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>);

    /// Executes the EVM with the given input and accepts a state hook closure that is invoked with
    /// the EVM state after execution.
    fn execute_with_state_hook<F>(
        self,
        input: Self::Input<'_>,
        state_hook: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook + 'static;
}

/// A general purpose executor that can execute multiple inputs in sequence, validate the outputs,
/// and keep track of the state over the entire batch.
pub trait BatchExecutor<DB> {
    /// The input type for the executor.
    type Input<'a>;
    /// The output type for the executor.
    type Output;
    /// The error type returned by the executor.
    type Error;

    /// Executes the next block in the batch, verifies the output and updates the state internally.
    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error>;

    /// Executes multiple inputs in the batch, verifies the output, and updates the state
    /// internally.
    ///
    /// This method is a convenience function for calling [`BatchExecutor::execute_and_verify_one`]
    /// for each input.
    fn execute_and_verify_many<'a, I>(&mut self, inputs: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Input<'a>>,
    {
        for input in inputs {
            self.execute_and_verify_one(input)?;
        }
        Ok(())
    }

    /// Executes the entire batch, verifies the output, and returns the final state.
    ///
    /// This method is a convenience function for calling [`BatchExecutor::execute_and_verify_many`]
    /// and [`BatchExecutor::finalize`].
    fn execute_and_verify_batch<'a, I>(mut self, batch: I) -> Result<Self::Output, Self::Error>
    where
        I: IntoIterator<Item = Self::Input<'a>>,
        Self: Sized,
    {
        self.execute_and_verify_many(batch)?;
        Ok(self.finalize())
    }

    /// Finishes the batch and return the final state.
    fn finalize(self) -> Self::Output;

    /// Set the expected tip of the batch.
    ///
    /// This can be used to optimize state pruning during execution.
    fn set_tip(&mut self, tip: BlockNumber);

    /// Set the prune modes.
    ///
    /// They are used to determine which parts of the state should be kept during execution.
    fn set_prune_modes(&mut self, prune_modes: PruneModes);

    /// The size hint of the batch's tracked state size.
    ///
    /// This is used to optimize DB commits depending on the size of the state.
    fn size_hint(&self) -> Option<usize>;
}

/// A type that can create a new executor for block execution.
pub trait BlockExecutorProvider: Send + Sync + Clone + Unpin + 'static {
    /// An executor that can execute a single block given a database.
    ///
    /// # Verification
    ///
    /// The on [`Executor::execute`] the executor is expected to validate the execution output of
    /// the input, this includes:
    /// - Cumulative gas used must match the input's gas used.
    /// - Receipts must match the input's receipts root.
    ///
    /// It is not expected to validate the state trie root, this must be done by the caller using
    /// the returned state.
    type Executor<DB: Database<Error: Into<ProviderError> + Display>>: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error = BlockExecutionError,
    >;

    /// An executor that can execute a batch of blocks given a database.
    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>>: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = ExecutionOutcome,
        Error = BlockExecutionError,
    >;

    /// Creates a new executor for single block execution.
    ///
    /// This is used to execute a single block and get the changed state.
    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>;

    /// Creates a new batch executor with the given database and pruning modes.
    ///
    /// Batch executor is used to execute multiple blocks in sequence and keep track of the state
    /// during historical sync which involves executing multiple blocks in sequence.
    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>;
}

/// Defines the strategy for executing a single block.
pub trait BlockExecutionStrategy<DB> {
    /// The error type returned by this strategy's methods.
    type Error: From<ProviderError> + core::error::Error;

    /// Applies any necessary changes before executing the block's transactions.
    fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error>;

    /// Executes all transactions in the block.
    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
    ) -> Result<(Vec<Receipt>, u64), Self::Error>;

    /// Applies any necessary changes after executing the block's transactions.
    fn apply_post_execution_changes(&mut self) -> Result<Vec<Request>, Self::Error>;

    /// Returns a reference to the current state.
    fn state_ref(&self) -> &State<DB>;

    /// Sets a hook to be called after each state change during execution.
    fn with_state_hook(self, hook: Option<Box<dyn OnStateHook>>) -> Self;

    /// Consumes the strategy and returns the final bundle state.
    fn finish(&self) -> BundleState;
}

/// A generic block executor that uses a [`BlockExecutionStrategy`] to
/// execute blocks.
#[allow(missing_debug_implementations, dead_code)]
pub struct GenericBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
{
    strategy: RefCell<S>,
    _phantom: PhantomData<DB>,
}

impl<S, DB> GenericBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
{
    /// Creates a new `GenericBlockExecutor` with the given strategy factory.
    pub const fn new(strategy: S) -> Self {
        Self { strategy: RefCell::new(strategy), _phantom: PhantomData }
    }
}

impl<S, DB> Executor<DB> for GenericBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = S::Error;

    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty: _ } = input;

        let mut strategy = self.strategy.borrow_mut();

        strategy.apply_pre_execution_changes()?;
        let (receipts, gas_used) = strategy.execute_transactions(block)?;
        let requests = strategy.apply_post_execution_changes()?;
        let state = strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }

    fn execute_with_state_witness<W>(
        self,
        input: Self::Input<'_>,
        mut witness: W,
    ) -> Result<Self::Output, Self::Error>
    where
        DB: Database,
        W: FnMut(&State<DB>),
    {
        let BlockExecutionInput { block, total_difficulty: _ } = input;

        let mut strategy = self.strategy.borrow_mut();

        strategy.apply_pre_execution_changes()?;
        let (receipts, gas_used) = strategy.execute_transactions(block)?;
        let requests = strategy.apply_post_execution_changes()?;

        witness(strategy.state_ref());

        let state = strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }

    fn execute_with_state_hook<H>(
        self,
        input: Self::Input<'_>,
        _state_hook: H,
    ) -> Result<Self::Output, Self::Error>
    where
        H: OnStateHook + 'static,
    {
        let BlockExecutionInput { block, total_difficulty: _ } = input;

        let mut strategy = self.strategy.borrow_mut();

        strategy.apply_pre_execution_changes()?;
        let (receipts, gas_used) = strategy.execute_transactions(block)?;
        let requests = strategy.apply_post_execution_changes()?;

        let state = strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip6110::DepositRequest;
    use alloy_primitives::U256;
    use reth_chainspec::{ChainSpec, MAINNET};
    use revm::db::{CacheDB, EmptyDBTyped};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct TestExecutorProvider;

    impl BlockExecutorProvider for TestExecutorProvider {
        type Executor<DB: Database<Error: Into<ProviderError> + Display>> = TestExecutor<DB>;
        type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> = TestExecutor<DB>;

        fn executor<DB>(&self, _db: DB) -> Self::Executor<DB>
        where
            DB: Database<Error: Into<ProviderError> + Display>,
        {
            TestExecutor(PhantomData)
        }

        fn batch_executor<DB>(&self, _db: DB) -> Self::BatchExecutor<DB>
        where
            DB: Database<Error: Into<ProviderError> + Display>,
        {
            TestExecutor(PhantomData)
        }
    }

    struct TestExecutor<DB>(PhantomData<DB>);

    impl<DB> Executor<DB> for TestExecutor<DB> {
        type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
        type Output = BlockExecutionOutput<Receipt>;
        type Error = BlockExecutionError;

        fn execute(self, _input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
            Err(BlockExecutionError::msg("execution unavailable for tests"))
        }

        fn execute_with_state_closure<F>(
            self,
            _: Self::Input<'_>,
            _: F,
        ) -> Result<Self::Output, Self::Error>
        where
            F: FnMut(&State<DB>),
        {
            Err(BlockExecutionError::msg("execution unavailable for tests"))
        }

        fn execute_with_state_hook<F>(
            self,
            _: Self::Input<'_>,
            _: F,
        ) -> Result<Self::Output, Self::Error>
        where
            F: OnStateHook,
        {
            Err(BlockExecutionError::msg("execution unavailable for tests"))
        }
    }

    impl<DB> BatchExecutor<DB> for TestExecutor<DB> {
        type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
        type Output = ExecutionOutcome;
        type Error = BlockExecutionError;

        fn execute_and_verify_one(&mut self, _input: Self::Input<'_>) -> Result<(), Self::Error> {
            Ok(())
        }

        fn finalize(self) -> Self::Output {
            todo!()
        }

        fn set_tip(&mut self, _tip: BlockNumber) {
            todo!()
        }

        fn set_prune_modes(&mut self, _prune_modes: PruneModes) {
            todo!()
        }

        fn size_hint(&self) -> Option<usize> {
            None
        }
    }

    struct TestExecutorStrategy<DB, EvmConfig> {
        // chain spec and evm config here only to illustrate how the strategy
        // factory can use them in a real use case.
        _chain_spec: Arc<ChainSpec>,
        _evm_config: EvmConfig,
        state: State<DB>,
        execute_transactions_result: (Vec<Receipt>, u64),
        apply_post_execution_changes_result: Vec<Request>,
        finish_result: BundleState,
    }

    impl<DB> BlockExecutionStrategy<DB> for TestExecutorStrategy<DB, TestEvmConfig> {
        type Error = BlockExecutionError;

        fn apply_pre_execution_changes(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn execute_transactions(
            &mut self,
            _block: &BlockWithSenders,
        ) -> Result<(Vec<Receipt>, u64), Self::Error> {
            Ok(self.execute_transactions_result.clone())
        }

        fn apply_post_execution_changes(&mut self) -> Result<Vec<Request>, Self::Error> {
            Ok(self.apply_post_execution_changes_result.clone())
        }

        fn state_ref(&self) -> &State<DB> {
            &self.state
        }

        fn with_state_hook(self, _hook: Option<Box<dyn OnStateHook>>) -> Self {
            self
        }

        fn finish(&self) -> BundleState {
            self.finish_result.clone()
        }
    }

    #[derive(Clone)]
    struct TestEvmConfig {}

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let executor = provider.executor(db);
        let _ = executor.execute(BlockExecutionInput::new(&Default::default(), U256::ZERO));
    }

    #[test]
    fn test_strategy() {
        let expected_gas_used = 10;
        let expected_receipts = vec![Receipt::default()];

        let expected_execute_transactions_result = (expected_receipts.clone(), expected_gas_used);
        let expected_apply_post_execution_changes_result =
            vec![Request::DepositRequest(DepositRequest::default())];
        let expected_finish_result = BundleState::default();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        let strategy = TestExecutorStrategy {
            _chain_spec: MAINNET.clone(),
            _evm_config: TestEvmConfig {},
            execute_transactions_result: expected_execute_transactions_result,
            apply_post_execution_changes_result: expected_apply_post_execution_changes_result
                .clone(),
            finish_result: expected_finish_result.clone(),
            state,
        };

        let executor = GenericBlockExecutor::new(strategy);
        let result = executor.execute(BlockExecutionInput::new(&Default::default(), U256::ZERO));
        assert!(result.is_ok());

        let block_execution_output = result.unwrap();

        assert_eq!(block_execution_output.gas_used, expected_gas_used);
        assert_eq!(block_execution_output.receipts, expected_receipts);
        assert_eq!(block_execution_output.requests, expected_apply_post_execution_changes_result);
        assert_eq!(block_execution_output.state, expected_finish_result);
    }
}
