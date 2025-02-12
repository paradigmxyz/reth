//! Traits for execution.

use alloy_consensus::BlockHeader;
// Re-export execution types
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::BlockExecutionResult;
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
pub use reth_storage_errors::provider::ProviderError;

use crate::{system_calls::OnStateHook, Database};
use alloc::{boxed::Box, vec::Vec};
use alloy_eips::eip7685::Requests;
use alloy_primitives::{
    map::{DefaultHashBuilder, HashMap},
    Address,
};
use reth_consensus::ConsensusError;
use reth_primitives::{NodePrimitives, Receipt, RecoveredBlock};
use revm::db::{states::bundle_state::BundleRetention, State};
use revm_primitives::{Account, AccountStatus, EvmState};

/// A type that knows how to execute a block. It is assumed to operate on a
/// [`crate::Evm`] internally and use [`State`] as database.
pub trait Executor<DB: Database>: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// The error type returned by the executor.
    type Error;

    /// Executes a single block and returns [`BlockExecutionResult`], without the state changes.
    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Executes the EVM with the given input and accepts a state hook closure that is invoked with
    /// the EVM state after execution.
    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static;

    /// Consumes the type and executes the block.
    ///
    /// # Note
    /// Execution happens without any validation of the output.
    ///
    /// # Returns
    /// The output of the block execution.
    fn execute(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let BlockExecutionResult { receipts, requests, gas_used } = self.execute_one(block)?;
        let mut state = self.into_state();
        Ok(BlockExecutionOutput { state: state.take_bundle(), receipts, requests, gas_used })
    }

    /// Executes multiple inputs in the batch, and returns an aggregated [`ExecutionOutcome`].
    fn execute_batch<'a, I>(
        mut self,
        blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>,
    {
        let mut results = Vec::new();
        let mut first_block = None;
        for block in blocks {
            if first_block.is_none() {
                first_block = Some(block.header().number());
            }
            results.push(self.execute_one(block)?);
        }

        Ok(ExecutionOutcome::from_blocks(
            first_block.unwrap_or_default(),
            self.into_state().take_bundle(),
            results,
        ))
    }

    /// Executes the EVM with the given input and accepts a state closure that is invoked with
    /// the EVM state after execution.
    fn execute_with_state_closure<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        mut f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        let BlockExecutionResult { receipts, requests, gas_used } = self.execute_one(block)?;
        let mut state = self.into_state();
        f(&state);
        Ok(BlockExecutionOutput { state: state.take_bundle(), receipts, requests, gas_used })
    }

    /// Executes the EVM with the given input and accepts a state hook closure that is invoked with
    /// the EVM state after execution.
    fn execute_with_state_hook<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        let BlockExecutionResult { receipts, requests, gas_used } =
            self.execute_one_with_state_hook(block, state_hook)?;
        let mut state = self.into_state();
        Ok(BlockExecutionOutput { state: state.take_bundle(), receipts, requests, gas_used })
    }

    /// Consumes the executor and returns the [`State`] containing all state changes.
    fn into_state(self) -> State<DB>;

    /// The size hint of the batch's tracked state size.
    ///
    /// This is used to optimize DB commits depending on the size of the state.
    fn size_hint(&self) -> usize;
}

/// A type that can create a new executor for block execution.
pub trait BlockExecutorProvider: Send + Sync + Clone + Unpin + 'static {
    /// Receipt type.
    type Primitives: NodePrimitives;

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
    type Executor<DB: Database>: Executor<
        DB,
        Primitives = Self::Primitives,
        Error = BlockExecutionError,
    >;

    /// Creates a new executor for single block execution.
    ///
    /// This is used to execute a single block and get the changed state.
    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database;
}

/// Helper type for the output of executing a block.
#[derive(Debug, Clone)]
pub struct ExecuteOutput<R = Receipt> {
    /// Receipts obtained after executing a block.
    pub receipts: Vec<R>,
    /// Cumulative gas used in the block execution.
    pub gas_used: u64,
}

/// Defines the strategy for executing a single block.
pub trait BlockExecutionStrategy {
    /// Database this strategy operates on.
    type DB: revm::Database;

    /// Primitive types used by the strategy.
    type Primitives: NodePrimitives;

    /// The error type returned by this strategy's methods.
    type Error: core::error::Error;

    /// Applies any necessary changes before executing the block's transactions.
    fn apply_pre_execution_changes(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<(), Self::Error>;

    /// Executes all transactions in the block.
    fn execute_transactions(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<ExecuteOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Applies any necessary changes after executing the block's transactions.
    fn apply_post_execution_changes(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        receipts: &[<Self::Primitives as NodePrimitives>::Receipt],
    ) -> Result<Requests, Self::Error>;

    /// Returns a reference to the current state.
    fn state_ref(&self) -> &State<Self::DB>;

    /// Returns a mutable reference to the current state.
    fn state_mut(&mut self) -> &mut State<Self::DB>;

    /// Consumes the strategy and returns inner [`State`].
    fn into_state(self) -> State<Self::DB>;

    /// Sets a hook to be called after each state change during execution.
    fn with_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

    /// Validate a block with regard to execution results.
    fn validate_block_post_execution(
        &self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _receipts: &[<Self::Primitives as NodePrimitives>::Receipt],
        _requests: &Requests,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

/// A strategy factory that can create block execution strategies.
pub trait BlockExecutionStrategyFactory: Send + Sync + Clone + Unpin + 'static {
    /// Primitive types used by the strategy.
    type Primitives: NodePrimitives;

    /// Associated strategy type.
    type Strategy<DB: Database>: BlockExecutionStrategy<
        DB = DB,
        Primitives = Self::Primitives,
        Error = BlockExecutionError,
    >;

    /// Creates a strategy using the give database.
    fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
    where
        DB: Database;
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
    type Primitives = F::Primitives;

    type Executor<DB: Database> = BasicBlockExecutor<F::Strategy<DB>>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database,
    {
        let strategy = self.strategy_factory.create_strategy(db);
        BasicBlockExecutor::new(strategy)
    }
}

/// A generic block executor that uses a [`BlockExecutionStrategy`] to
/// execute blocks.
#[allow(missing_debug_implementations, dead_code)]
pub struct BasicBlockExecutor<S> {
    /// Block execution strategy.
    pub(crate) strategy: S,
}

impl<S> BasicBlockExecutor<S> {
    /// Creates a new `BasicBlockExecutor` with the given strategy.
    pub const fn new(strategy: S) -> Self {
        Self { strategy }
    }
}

impl<S, DB> Executor<DB> for BasicBlockExecutor<S>
where
    S: BlockExecutionStrategy<DB = DB>,
    DB: Database,
{
    type Primitives = S::Primitives;
    type Error = S::Error;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        self.strategy.apply_pre_execution_changes(block)?;
        let ExecuteOutput { receipts, gas_used } = self.strategy.execute_transactions(block)?;
        let requests = self.strategy.apply_post_execution_changes(block, &receipts)?;
        self.strategy.state_mut().merge_transitions(BundleRetention::Reverts);

        Ok(BlockExecutionResult { receipts, requests, gas_used })
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        self.strategy.with_state_hook(Some(Box::new(state_hook)));
        let result = self.execute_one(block);
        self.strategy.with_state_hook(None);

        result
    }

    fn into_state(self) -> State<DB> {
        self.strategy.into_state()
    }

    fn size_hint(&self) -> usize {
        self.strategy.state_ref().bundle_state.size_hint()
    }
}

/// Creates an `EvmState` from a map of balance increments and the current state
/// to load accounts from. No balance increment is done in the function.
/// Zero balance increments are ignored and won't create state entries.
pub fn balance_increment_state<DB>(
    balance_increments: &HashMap<Address, u128, DefaultHashBuilder>,
    state: &mut State<DB>,
) -> Result<EvmState, BlockExecutionError>
where
    DB: Database,
{
    let mut load_account = |address: &Address| -> Result<(Address, Account), BlockExecutionError> {
        let cache_account = state.load_cache_account(*address).map_err(|_| {
            BlockExecutionError::msg("could not load account for balance increment")
        })?;

        let account = cache_account.account.as_ref().ok_or_else(|| {
            BlockExecutionError::msg("could not load account for balance increment")
        })?;

        Ok((
            *address,
            Account {
                info: account.info.clone(),
                storage: Default::default(),
                status: AccountStatus::Touched,
            },
        ))
    };

    balance_increments
        .iter()
        .filter(|(_, &balance)| balance != 0)
        .map(|(addr, _)| load_account(addr))
        .collect::<Result<EvmState, _>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use core::marker::PhantomData;
    use reth_chainspec::{ChainSpec, MAINNET};
    use reth_primitives::EthPrimitives;
    use revm::db::{CacheDB, EmptyDBTyped};
    use revm_primitives::{address, bytes, AccountInfo, KECCAK_EMPTY};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct TestExecutorProvider;

    impl BlockExecutorProvider for TestExecutorProvider {
        type Primitives = EthPrimitives;
        type Executor<DB: Database> = TestExecutor<DB>;

        fn executor<DB>(&self, _db: DB) -> Self::Executor<DB>
        where
            DB: Database,
        {
            TestExecutor(PhantomData)
        }
    }

    struct TestExecutor<DB>(PhantomData<DB>);

    impl<DB: Database> Executor<DB> for TestExecutor<DB> {
        type Primitives = EthPrimitives;
        type Error = BlockExecutionError;

        fn execute_one(
            &mut self,
            _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
        {
            Err(BlockExecutionError::msg("execution unavailable for tests"))
        }

        fn execute_one_with_state_hook<F>(
            &mut self,
            _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
            _state_hook: F,
        ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
        where
            F: OnStateHook + 'static,
        {
            Err(BlockExecutionError::msg("execution unavailable for tests"))
        }

        fn into_state(self) -> State<DB> {
            unreachable!()
        }

        fn size_hint(&self) -> usize {
            0
        }
    }

    struct TestExecutorStrategy<DB, EvmConfig> {
        // chain spec and evm config here only to illustrate how the strategy
        // factory can use them in a real use case.
        _chain_spec: Arc<ChainSpec>,
        _evm_config: EvmConfig,
        state: State<DB>,
        execute_transactions_result: ExecuteOutput<Receipt>,
        apply_post_execution_changes_result: Requests,
    }

    #[derive(Clone)]
    struct TestExecutorStrategyFactory {
        execute_transactions_result: ExecuteOutput<Receipt>,
        apply_post_execution_changes_result: Requests,
    }

    impl BlockExecutionStrategyFactory for TestExecutorStrategyFactory {
        type Primitives = EthPrimitives;
        type Strategy<DB: Database> = TestExecutorStrategy<DB, TestEvmConfig>;

        fn create_strategy<DB>(&self, db: DB) -> Self::Strategy<DB>
        where
            DB: Database,
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
                state,
            }
        }
    }

    impl<DB> BlockExecutionStrategy for TestExecutorStrategy<DB, TestEvmConfig>
    where
        DB: Database,
    {
        type DB = DB;
        type Primitives = EthPrimitives;
        type Error = BlockExecutionError;

        fn apply_pre_execution_changes(
            &mut self,
            _block: &RecoveredBlock<reth_primitives::Block>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn execute_transactions(
            &mut self,
            _block: &RecoveredBlock<reth_primitives::Block>,
        ) -> Result<ExecuteOutput<Receipt>, Self::Error> {
            Ok(self.execute_transactions_result.clone())
        }

        fn apply_post_execution_changes(
            &mut self,
            _block: &RecoveredBlock<reth_primitives::Block>,
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

        fn into_state(self) -> State<Self::DB> {
            self.state
        }
    }

    #[derive(Clone)]
    struct TestEvmConfig {}

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let executor = provider.executor(db);
        let _ = executor.execute(&Default::default());
    }

    #[test]
    fn test_strategy() {
        let expected_gas_used = 10;
        let expected_receipts = vec![Receipt::default()];
        let expected_execute_transactions_result = ExecuteOutput::<Receipt> {
            receipts: expected_receipts.clone(),
            gas_used: expected_gas_used,
        };
        let expected_apply_post_execution_changes_result = Requests::new(vec![bytes!("deadbeef")]);

        let strategy_factory = TestExecutorStrategyFactory {
            execute_transactions_result: expected_execute_transactions_result,
            apply_post_execution_changes_result: expected_apply_post_execution_changes_result
                .clone(),
        };
        let provider = BasicBlockExecutorProvider::new(strategy_factory);
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let executor = provider.executor(db);
        let result = executor.execute(&Default::default());

        assert!(result.is_ok());
        let block_execution_output = result.unwrap();
        assert_eq!(block_execution_output.gas_used, expected_gas_used);
        assert_eq!(block_execution_output.receipts, expected_receipts);
        assert_eq!(block_execution_output.requests, expected_apply_post_execution_changes_result);
    }

    fn setup_state_with_account(
        addr: Address,
        balance: u128,
        nonce: u64,
    ) -> State<CacheDB<EmptyDBTyped<BlockExecutionError>>> {
        let db = CacheDB::<EmptyDBTyped<BlockExecutionError>>::default();
        let mut state = State::builder().with_database(db).with_bundle_update().build();

        let account_info = AccountInfo {
            balance: U256::from(balance),
            nonce,
            code_hash: KECCAK_EMPTY,
            code: None,
        };
        state.insert_account(addr, account_info);
        state
    }

    #[test]
    fn test_balance_increment_state_zero() {
        let addr = address!("1000000000000000000000000000000000000000");
        let mut state = setup_state_with_account(addr, 100, 1);

        let mut increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
        increments.insert(addr, 0);

        let result = balance_increment_state(&increments, &mut state).unwrap();
        assert!(result.is_empty(), "Zero increments should be ignored");
    }

    #[test]
    fn test_balance_increment_state_empty_increments_map() {
        let mut state = State::builder()
            .with_database(CacheDB::<EmptyDBTyped<BlockExecutionError>>::default())
            .with_bundle_update()
            .build();

        let increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
        let result = balance_increment_state(&increments, &mut state).unwrap();
        assert!(result.is_empty(), "Empty increments map should return empty state");
    }

    #[test]
    fn test_balance_increment_state_multiple_valid_increments() {
        let addr1 = address!("1000000000000000000000000000000000000000");
        let addr2 = address!("2000000000000000000000000000000000000000");

        let mut state = setup_state_with_account(addr1, 100, 1);

        let account2 =
            AccountInfo { balance: U256::from(200), nonce: 1, code_hash: KECCAK_EMPTY, code: None };
        state.insert_account(addr2, account2);

        let mut increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
        increments.insert(addr1, 50);
        increments.insert(addr2, 100);

        let result = balance_increment_state(&increments, &mut state).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(&addr1).unwrap().info.balance, U256::from(100));
        assert_eq!(result.get(&addr2).unwrap().info.balance, U256::from(200));
    }

    #[test]
    fn test_balance_increment_state_mixed_zero_and_nonzero_increments() {
        let addr1 = address!("1000000000000000000000000000000000000000");
        let addr2 = address!("2000000000000000000000000000000000000000");

        let mut state = setup_state_with_account(addr1, 100, 1);

        let account2 =
            AccountInfo { balance: U256::from(200), nonce: 1, code_hash: KECCAK_EMPTY, code: None };
        state.insert_account(addr2, account2);

        let mut increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
        increments.insert(addr1, 0);
        increments.insert(addr2, 100);

        let result = balance_increment_state(&increments, &mut state).unwrap();

        assert_eq!(result.len(), 1, "Only non-zero increments should be included");
        assert!(!result.contains_key(&addr1), "Zero increment account should not be included");
        assert_eq!(result.get(&addr2).unwrap().info.balance, U256::from(200));
    }
}
