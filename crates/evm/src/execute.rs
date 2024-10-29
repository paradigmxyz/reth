//! Traits for execution.

// Re-export execution types
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
pub use reth_execution_types::{BlockExecutionInput, BlockExecutionOutput, ExecutionOutcome};
pub use reth_storage_errors::provider::ProviderError;
use revm::db::states::bundle_state::BundleRetention;

use crate::system_calls::OnStateHook;
use alloc::{boxed::Box, vec::Vec};
use alloy_eips::eip7685::Requests;
use alloy_primitives::BlockNumber;
use core::{fmt::Display, marker::PhantomData};
use reth_consensus::ConsensusError;
use reth_primitives::{BlockWithSenders, Receipt};
use reth_prune_types::PruneModes;
use reth_revm::batch::BlockBatchRecord;
use revm::{db::BundleState, State};
use revm_primitives::{db::Database, U256};

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

/// Helper type for the output of executing a block.
#[derive(Debug, Clone)]
pub struct ExecuteOutput {
    /// Receipts obtained after executing a block.
    pub receipts: Vec<Receipt>,
    /// Cumulative gas used in the block execution.
    pub gas_used: u64,
}

/// Defines the strategy for executing a single block.
pub trait BlockExecutionStrategy<DB>
where
    DB: Database,
{
    /// The error type returned by this strategy's methods.
    type Error: From<ProviderError> + core::error::Error;

    /// Applies any necessary changes before executing the block's transactions.
    fn apply_pre_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), Self::Error>;

    /// Executes all transactions in the block.
    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<ExecuteOutput, Self::Error>;

    /// Applies any necessary changes after executing the block's transactions.
    fn apply_post_execution_changes(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
        receipts: &[Receipt],
    ) -> Result<Requests, Self::Error>;

    /// Returns a reference to the current state.
    fn state_ref(&self) -> &State<DB>;

    /// Returns a mutable reference to the current state.
    fn state_mut(&mut self) -> &mut State<DB>;

    /// Sets a hook to be called after each state change during execution.
    fn with_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

    /// Returns the final bundle state.
    fn finish(&mut self) -> BundleState {
        self.state_mut().merge_transitions(BundleRetention::Reverts);
        self.state_mut().take_bundle()
    }

    /// Validate a block with regard to execution results.
    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders,
        _receipts: &[Receipt],
        _requests: &Requests,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

/// A strategy factory that can create block execution strategies.
pub trait BlockExecutionStrategyFactory: Send + Sync + Clone + Unpin + 'static {
    /// Associated strategy type.
    type Strategy<DB: Database<Error: Into<ProviderError> + Display>>: BlockExecutionStrategy<
        DB,
        Error = BlockExecutionError,
    >;

    /// Creates a strategy using the give database.
    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>;
}

impl<F> Clone for BasicBlockExecutorProvider<F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self { strategy_factory: self.strategy_factory.clone() }
    }
}

/// A generic block executor provider that can create executors using a strategy factory.
#[allow(missing_debug_implementations)]
pub struct BasicBlockExecutorProvider<F> {
    strategy_factory: F,
}

impl<F> BasicBlockExecutorProvider<F> {
    /// Creates a new `BasicBlockExecutorProvider` with the given strategy factory.
    pub const fn new(strategy_factory: F) -> Self {
        Self { strategy_factory }
    }
}

impl<F> BlockExecutorProvider for BasicBlockExecutorProvider<F>
where
    F: BlockExecutionStrategyFactory,
{
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> =
        BasicBlockExecutor<F::Strategy<DB>, DB>;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> =
        BasicBatchExecutor<F::Strategy<DB>, DB>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let strategy = self.strategy_factory.create_strategy(db);
        BasicBlockExecutor::new(strategy)
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        let strategy = self.strategy_factory.create_strategy(db);
        let batch_record = BlockBatchRecord::default();
        BasicBatchExecutor::new(strategy, batch_record)
    }
}

/// A generic block executor that uses a [`BlockExecutionStrategy`] to
/// execute blocks.
#[allow(missing_debug_implementations, dead_code)]
pub struct BasicBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database,
{
    /// Block execution strategy.
    pub(crate) strategy: S,
    _phantom: PhantomData<DB>,
}

impl<S, DB> BasicBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database,
{
    /// Creates a new `BasicBlockExecutor` with the given strategy.
    pub const fn new(strategy: S) -> Self {
        Self { strategy, _phantom: PhantomData }
    }
}

impl<S, DB> Executor<DB> for BasicBlockExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = S::Error;

    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;

        self.strategy.apply_pre_execution_changes(block, total_difficulty)?;
        let ExecuteOutput { receipts, gas_used } =
            self.strategy.execute_transactions(block, total_difficulty)?;
        let requests =
            self.strategy.apply_post_execution_changes(block, total_difficulty, &receipts)?;
        let state = self.strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }

    fn execute_with_state_closure<F>(
        mut self,
        input: Self::Input<'_>,
        mut state: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        let BlockExecutionInput { block, total_difficulty } = input;

        self.strategy.apply_pre_execution_changes(block, total_difficulty)?;
        let ExecuteOutput { receipts, gas_used } =
            self.strategy.execute_transactions(block, total_difficulty)?;
        let requests =
            self.strategy.apply_post_execution_changes(block, total_difficulty, &receipts)?;

        state(self.strategy.state_ref());

        let state = self.strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }

    fn execute_with_state_hook<H>(
        mut self,
        input: Self::Input<'_>,
        state_hook: H,
    ) -> Result<Self::Output, Self::Error>
    where
        H: OnStateHook + 'static,
    {
        let BlockExecutionInput { block, total_difficulty } = input;

        self.strategy.with_state_hook(Some(Box::new(state_hook)));

        self.strategy.apply_pre_execution_changes(block, total_difficulty)?;
        let ExecuteOutput { receipts, gas_used } =
            self.strategy.execute_transactions(block, total_difficulty)?;
        let requests =
            self.strategy.apply_post_execution_changes(block, total_difficulty, &receipts)?;

        let state = self.strategy.finish();

        Ok(BlockExecutionOutput { state, receipts, requests, gas_used })
    }
}

/// A generic batch executor that uses a [`BlockExecutionStrategy`] to
/// execute batches.
#[allow(missing_debug_implementations)]
pub struct BasicBatchExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database,
{
    /// Batch execution strategy.
    pub(crate) strategy: S,
    /// Keeps track of batch execution receipts and requests.
    pub(crate) batch_record: BlockBatchRecord,
    _phantom: PhantomData<DB>,
}

impl<S, DB> BasicBatchExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB>,
    DB: Database,
{
    /// Creates a new `BasicBatchExecutor` with the given strategy.
    pub const fn new(strategy: S, batch_record: BlockBatchRecord) -> Self {
        Self { strategy, batch_record, _phantom: PhantomData }
    }
}

impl<S, DB> BatchExecutor<DB> for BasicBatchExecutor<S, DB>
where
    S: BlockExecutionStrategy<DB, Error = BlockExecutionError>,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        let BlockExecutionInput { block, total_difficulty } = input;

        if self.batch_record.first_block().is_none() {
            self.batch_record.set_first_block(block.number);
        }

        self.strategy.apply_pre_execution_changes(block, total_difficulty)?;
        let ExecuteOutput { receipts, .. } =
            self.strategy.execute_transactions(block, total_difficulty)?;
        let requests =
            self.strategy.apply_post_execution_changes(block, total_difficulty, &receipts)?;

        self.strategy.validate_block_post_execution(block, &receipts, &requests)?;

        // prepare the state according to the prune mode
        let retention = self.batch_record.bundle_retention(block.number);
        self.strategy.state_mut().merge_transitions(retention);

        // store receipts in the set
        self.batch_record.save_receipts(receipts)?;

        // store requests in the set
        self.batch_record.save_requests(requests);

        Ok(())
    }

    fn finalize(mut self) -> Self::Output {
        ExecutionOutcome::new(
            self.strategy.state_mut().take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
            self.batch_record.take_requests(),
        )
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        self.batch_record.set_tip(tip);
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.batch_record.set_prune_modes(prune_modes);
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.strategy.state_ref().bundle_state.size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_chainspec::{ChainSpec, MAINNET};
    use revm::db::{CacheDB, EmptyDBTyped};
    use revm_primitives::bytes;
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
        execute_transactions_result: ExecuteOutput,
        apply_post_execution_changes_result: Requests,
        finish_result: BundleState,
    }

    #[derive(Clone)]
    struct TestExecutorStrategyFactory {
        execute_transactions_result: ExecuteOutput,
        apply_post_execution_changes_result: Requests,
        finish_result: BundleState,
    }

    impl BlockExecutionStrategyFactory for TestExecutorStrategyFactory {
        type Strategy<DB: Database<Error: Into<ProviderError> + Display>> =
            TestExecutorStrategy<DB, TestEvmConfig>;

        fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
        where
            DB: Database<Error: Into<ProviderError> + Display>,
        {
            let state = State::builder()
                .with_database(db)
                .with_bundle_update()
                .without_state_clear()
                .build();

            TestExecutorStrategy {
                _chain_spec: MAINNET.clone(),
                _evm_config: TestEvmConfig {},
                execute_transactions_result: self.execute_transactions_result.clone(),
                apply_post_execution_changes_result: self
                    .apply_post_execution_changes_result
                    .clone(),
                finish_result: self.finish_result.clone(),
                state,
            }
        }
    }

    impl<DB> BlockExecutionStrategy<DB> for TestExecutorStrategy<DB, TestEvmConfig>
    where
        DB: Database,
    {
        type Error = BlockExecutionError;

        fn apply_pre_execution_changes(
            &mut self,
            _block: &BlockWithSenders,
            _total_difficulty: U256,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn execute_transactions(
            &mut self,
            _block: &BlockWithSenders,
            _total_difficulty: U256,
        ) -> Result<ExecuteOutput, Self::Error> {
            Ok(self.execute_transactions_result.clone())
        }

        fn apply_post_execution_changes(
            &mut self,
            _block: &BlockWithSenders,
            _total_difficulty: U256,
            _receipts: &[Receipt],
        ) -> Result<Requests, Self::Error> {
            Ok(self.apply_post_execution_changes_result.clone())
        }

        fn state_ref(&self) -> &State<DB> {
            &self.state
        }

        fn state_mut(&mut self) -> &mut State<DB> {
            &mut self.state
        }

        fn with_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

        fn finish(&mut self) -> BundleState {
            self.finish_result.clone()
        }

        fn validate_block_post_execution(
            &self,
            _block: &BlockWithSenders,
            _receipts: &[Receipt],
            _requests: &Requests,
        ) -> Result<(), ConsensusError> {
            Ok(())
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
        let expected_execute_transactions_result =
            ExecuteOutput { receipts: expected_receipts.clone(), gas_used: expected_gas_used };
        let expected_apply_post_execution_changes_result = Requests::new(vec![bytes!("deadbeef")]);
        let expected_finish_result = BundleState::default();

        let strategy_factory = TestExecutorStrategyFactory {
            execute_transactions_result: expected_execute_transactions_result,
            apply_post_execution_changes_result: expected_apply_post_execution_changes_result
                .clone(),
            finish_result: expected_finish_result.clone(),
        };
        let provider = BasicBlockExecutorProvider::new(strategy_factory);
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let executor = provider.executor(db);
        let result = executor.execute(BlockExecutionInput::new(&Default::default(), U256::ZERO));

        assert!(result.is_ok());
        let block_execution_output = result.unwrap();
        assert_eq!(block_execution_output.gas_used, expected_gas_used);
        assert_eq!(block_execution_output.receipts, expected_receipts);
        assert_eq!(block_execution_output.requests, expected_apply_post_execution_changes_result);
        assert_eq!(block_execution_output.state, expected_finish_result);
    }
}
