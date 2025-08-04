//! Traits for execution.

use crate::{ConfigureEvm, Database, OnStateHook, TxEnvFor};
use alloc::{boxed::Box, vec::Vec};
use alloy_consensus::{BlockHeader, Header};
use alloy_eips::eip2718::WithEncoded;
pub use alloy_evm::block::{BlockExecutor, BlockExecutorFactory};
use alloy_evm::{
    block::{CommitChanges, ExecutableTx},
    Evm, EvmEnv, EvmFactory, RecoveredTx, ToTxEnv,
};
use alloy_primitives::{Address, B256};
use core::fmt::Debug;
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::BlockExecutionResult;
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, Recovered, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
pub use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::{
    context::result::ExecutionResult,
    database::{states::bundle_state::BundleRetention, BundleState, State},
};

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
        let result = self.execute_one(block)?;
        let mut state = self.into_state();
        Ok(BlockExecutionOutput { state: state.take_bundle(), result })
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
        let result = self.execute_one(block)?;
        let mut state = self.into_state();
        f(&state);
        Ok(BlockExecutionOutput { state: state.take_bundle(), result })
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
        let result = self.execute_one_with_state_hook(block, state_hook)?;
        let mut state = self.into_state();
        Ok(BlockExecutionOutput { state: state.take_bundle(), result })
    }

    /// Consumes the executor and returns the [`State`] containing all state changes.
    fn into_state(self) -> State<DB>;

    /// The size hint of the batch's tracked state size.
    ///
    /// This is used to optimize DB commits depending on the size of the state.
    fn size_hint(&self) -> usize;
}

/// Helper type for the output of executing a block.
#[derive(Debug, Clone)]
pub struct ExecuteOutput<R> {
    /// Receipts obtained after executing a block.
    pub receipts: Vec<R>,
    /// Cumulative gas used in the block execution.
    pub gas_used: u64,
}

/// Input for block building. Consumed by [`BlockAssembler`].
///
/// This struct contains all the data needed by the [`BlockAssembler`] to create
/// a complete block after transaction execution.
///
/// # Fields Overview
///
/// - `evm_env`: The EVM configuration used during execution (spec ID, block env, etc.)
/// - `execution_ctx`: Additional context like withdrawals and ommers
/// - `parent`: The parent block header this block builds on
/// - `transactions`: All transactions that were successfully executed
/// - `output`: Execution results including receipts and gas used
/// - `bundle_state`: Accumulated state changes from all transactions
/// - `state_provider`: Access to the current state for additional lookups
/// - `state_root`: The calculated state root after all changes
///
/// # Usage
///
/// This is typically created internally by [`BlockBuilder::finish`] after all
/// transactions have been executed:
///
/// ```rust,ignore
/// let input = BlockAssemblerInput {
///     evm_env: builder.evm_env(),
///     execution_ctx: builder.context(),
///     parent: &parent_header,
///     transactions: executed_transactions,
///     output: &execution_result,
///     bundle_state: &state_changes,
///     state_provider: &state,
///     state_root: calculated_root,
/// };
///
/// let block = assembler.assemble_block(input)?;
/// ```
#[derive(derive_more::Debug)]
#[non_exhaustive]
pub struct BlockAssemblerInput<'a, 'b, F: BlockExecutorFactory, H = Header> {
    /// Configuration of EVM used when executing the block.
    ///
    /// Contains context relevant to EVM such as [`revm::context::BlockEnv`].
    pub evm_env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec>,
    /// [`BlockExecutorFactory::ExecutionCtx`] used to execute the block.
    pub execution_ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<H>,
    /// Transactions that were executed in this block.
    pub transactions: Vec<F::Transaction>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<F::Receipt>,
    /// [`BundleState`] after the block execution.
    pub bundle_state: &'a BundleState,
    /// Provider with access to state.
    #[debug(skip)]
    pub state_provider: &'b dyn StateProvider,
    /// State root for this block.
    pub state_root: B256,
}

/// A type that knows how to assemble a block from execution results.
///
/// The [`BlockAssembler`] is the final step in block production. After transactions
/// have been executed by the [`BlockExecutor`], the assembler takes all the execution
/// outputs and creates a properly formatted block.
///
/// # Responsibilities
///
/// The assembler is responsible for:
/// - Setting the correct block header fields (gas used, receipts root, logs bloom, etc.)
/// - Including the executed transactions in the correct order
/// - Setting the state root from the post-execution state
/// - Applying any chain-specific rules or adjustments
///
/// # Example Flow
///
/// ```rust,ignore
/// // 1. Execute transactions and get results
/// let execution_result = block_executor.finish()?;
///
/// // 2. Calculate state root from changes
/// let state_root = state_provider.state_root(&bundle_state)?;
///
/// // 3. Assemble the final block
/// let block = assembler.assemble_block(BlockAssemblerInput {
///     evm_env,           // Environment used during execution
///     execution_ctx,     // Context like withdrawals, ommers
///     parent,            // Parent block header
///     transactions,      // Executed transactions
///     output,            // Execution results (receipts, gas)
///     bundle_state,      // All state changes
///     state_provider,    // For additional lookups if needed
///     state_root,        // Computed state root
/// })?;
/// ```
///
/// # Relationship with Block Building
///
/// The assembler works together with:
/// - `NextBlockEnvAttributes`: Provides the configuration for the new block
/// - [`BlockExecutor`]: Executes transactions and produces results
/// - [`BlockBuilder`]: Orchestrates the entire process and calls the assembler
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockAssembler<F: BlockExecutorFactory> {
    /// The block type produced by the assembler.
    type Block: Block;

    /// Builds a block. see [`BlockAssemblerInput`] documentation for more details.
    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F, <Self::Block as Block>::Header>,
    ) -> Result<Self::Block, BlockExecutionError>;
}

/// Output of block building.
#[derive(Debug, Clone)]
pub struct BlockBuilderOutcome<N: NodePrimitives> {
    /// Result of block execution.
    pub execution_result: BlockExecutionResult<N::Receipt>,
    /// Hashed state after execution.
    pub hashed_state: HashedPostState,
    /// Trie updates collected during state root calculation.
    pub trie_updates: TrieUpdates,
    /// The built block.
    pub block: RecoveredBlock<N::Block>,
}

/// A type that knows how to execute and build a block.
///
/// It wraps an inner [`BlockExecutor`] and provides a way to execute transactions and
/// construct a block.
///
/// This is a helper to erase `BasicBlockBuilder` type.
pub trait BlockBuilder {
    /// The primitive types used by the inner [`BlockExecutor`].
    type Primitives: NodePrimitives;
    /// Inner [`BlockExecutor`].
    type Executor: BlockExecutor<
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
    >;

    /// Invokes [`BlockExecutor::apply_pre_execution_changes`].
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Invokes [`BlockExecutor::execute_transaction_with_commit_condition`] and saves the
    /// transaction in internal state only if the transaction was committed.
    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(
            &ExecutionResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::HaltReason>,
        ) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError>;

    /// Invokes [`BlockExecutor::execute_transaction_with_result_closure`] and saves the
    /// transaction in internal state.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&ExecutionResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(tx, |res| {
            f(res);
            CommitChanges::Yes
        })
        .map(Option::unwrap_or_default)
    }

    /// Invokes [`BlockExecutor::execute_transaction`] and saves the transaction in
    /// internal state.
    fn execute_transaction(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
    ) -> Result<u64, BlockExecutionError> {
        self.execute_transaction_with_result_closure(tx, |_| ())
    }

    /// Completes the block building process and returns the [`BlockBuilderOutcome`].
    fn finish(
        self,
        state_provider: impl StateProvider,
    ) -> Result<BlockBuilderOutcome<Self::Primitives>, BlockExecutionError>;

    /// Provides mutable access to the inner [`BlockExecutor`].
    fn executor_mut(&mut self) -> &mut Self::Executor;

    /// Provides access to the inner [`BlockExecutor`].
    fn executor(&self) -> &Self::Executor;

    /// Helper to access inner [`BlockExecutor::Evm`] mutably.
    fn evm_mut(&mut self) -> &mut <Self::Executor as BlockExecutor>::Evm {
        self.executor_mut().evm_mut()
    }

    /// Helper to access inner [`BlockExecutor::Evm`].
    fn evm(&self) -> &<Self::Executor as BlockExecutor>::Evm {
        self.executor().evm()
    }

    /// Consumes the type and returns the underlying [`BlockExecutor`].
    fn into_executor(self) -> Self::Executor;
}

/// A type that constructs a block from transactions and execution results.
#[derive(Debug)]
pub struct BasicBlockBuilder<'a, F, Executor, Builder, N: NodePrimitives>
where
    F: BlockExecutorFactory,
{
    /// The block executor used to execute transactions.
    pub executor: Executor,
    /// The transactions executed in this block.
    pub transactions: Vec<Recovered<TxTy<N>>>,
    /// The parent block execution context.
    pub ctx: F::ExecutionCtx<'a>,
    /// The sealed parent block header.
    pub parent: &'a SealedHeader<HeaderTy<N>>,
    /// The assembler used to build the block.
    pub assembler: Builder,
}

/// Conversions for executable transactions.
pub trait ExecutorTx<Executor: BlockExecutor> {
    /// Converts the transaction into [`ExecutableTx`].
    fn as_executable(&self) -> impl ExecutableTx<Executor>;

    /// Converts the transaction into [`Recovered`].
    fn into_recovered(self) -> Recovered<Executor::Transaction>;
}

impl<Executor: BlockExecutor> ExecutorTx<Executor>
    for WithEncoded<Recovered<Executor::Transaction>>
{
    fn as_executable(&self) -> impl ExecutableTx<Executor> {
        self
    }

    fn into_recovered(self) -> Recovered<Executor::Transaction> {
        self.1
    }
}

impl<Executor: BlockExecutor> ExecutorTx<Executor> for Recovered<Executor::Transaction> {
    fn as_executable(&self) -> impl ExecutableTx<Executor> {
        self
    }

    fn into_recovered(self) -> Self {
        self
    }
}

impl<'a, F, DB, Executor, Builder, N> BlockBuilder
    for BasicBlockBuilder<'a, F, Executor, Builder, N>
where
    F: BlockExecutorFactory<Transaction = N::SignedTx, Receipt = N::Receipt>,
    Executor: BlockExecutor<
        Evm: Evm<
            Spec = <F::EvmFactory as EvmFactory>::Spec,
            HaltReason = <F::EvmFactory as EvmFactory>::HaltReason,
            DB = &'a mut State<DB>,
        >,
        Transaction = N::SignedTx,
        Receipt = N::Receipt,
    >,
    DB: Database + 'a,
    Builder: BlockAssembler<F, Block = N::Block>,
    N: NodePrimitives,
{
    type Primitives = N;
    type Executor = Executor;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.executor.apply_pre_execution_changes()
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(
            &ExecutionResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::HaltReason>,
        ) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        if let Some(gas_used) =
            self.executor.execute_transaction_with_commit_condition(tx.as_executable(), f)?
        {
            self.transactions.push(tx.into_recovered());
            Ok(Some(gas_used))
        } else {
            Ok(None)
        }
    }

    fn finish(
        self,
        state: impl StateProvider,
    ) -> Result<BlockBuilderOutcome<N>, BlockExecutionError> {
        let (evm, result) = self.executor.finish()?;
        let (db, evm_env) = evm.finish();

        // merge all transitions into bundle state
        db.merge_transitions(BundleRetention::Reverts);

        // calculate the state root
        let hashed_state = state.hashed_post_state(&db.bundle_state);
        let (state_root, trie_updates) = state
            .state_root_with_updates(hashed_state.clone())
            .map_err(BlockExecutionError::other)?;

        let (transactions, senders) =
            self.transactions.into_iter().map(|tx| tx.into_parts()).unzip();

        let block = self.assembler.assemble_block(BlockAssemblerInput {
            evm_env,
            execution_ctx: self.ctx,
            parent: self.parent,
            transactions,
            output: &result,
            bundle_state: &db.bundle_state,
            state_provider: &state,
            state_root,
        })?;

        let block = RecoveredBlock::new_unhashed(block, senders);

        Ok(BlockBuilderOutcome { execution_result: result, hashed_state, trie_updates, block })
    }

    fn executor_mut(&mut self) -> &mut Self::Executor {
        &mut self.executor
    }

    fn executor(&self) -> &Self::Executor {
        &self.executor
    }

    fn into_executor(self) -> Self::Executor {
        self.executor
    }
}

/// A generic block executor that uses a [`BlockExecutor`] to
/// execute blocks.
#[expect(missing_debug_implementations)]
pub struct BasicBlockExecutor<F, DB> {
    /// Block execution strategy.
    pub(crate) strategy_factory: F,
    /// Database.
    pub(crate) db: State<DB>,
}

impl<F, DB: Database> BasicBlockExecutor<F, DB> {
    /// Creates a new `BasicBlockExecutor` with the given strategy.
    pub fn new(strategy_factory: F, db: DB) -> Self {
        let db =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        Self { strategy_factory, db }
    }
}

impl<F, DB> Executor<DB> for BasicBlockExecutor<F, DB>
where
    F: ConfigureEvm,
    DB: Database,
{
    type Primitives = F::Primitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let result = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .execute_block(block.transactions_recovered())?;

        self.db.merge_transitions(BundleRetention::Reverts);

        Ok(result)
    }

    fn execute_one_with_state_hook<H>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: H,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        H: OnStateHook + 'static,
    {
        let result = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .with_state_hook(Some(Box::new(state_hook)))
            .execute_block(block.transactions_recovered())?;

        self.db.merge_transitions(BundleRetention::Reverts);

        Ok(result)
    }

    fn into_state(self) -> State<DB> {
        self.db
    }

    fn size_hint(&self) -> usize {
        self.db.bundle_state.size_hint()
    }
}

/// A helper trait marking a 'static type that can be converted into an [`ExecutableTx`] for block
/// executor.
pub trait ExecutableTxFor<Evm: ConfigureEvm>:
    ToTxEnv<TxEnvFor<Evm>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

impl<T, Evm: ConfigureEvm> ExecutableTxFor<Evm> for T where
    T: ToTxEnv<TxEnvFor<Evm>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

/// A container for a transaction and a transaction environment.
#[derive(Debug, Clone)]
pub struct WithTxEnv<TxEnv, T> {
    /// The transaction environment for EVM.
    pub tx_env: TxEnv,
    /// The recovered transaction.
    pub tx: T,
}

impl<TxEnv, Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithTxEnv<TxEnv, T> {
    fn tx(&self) -> &Tx {
        self.tx.tx()
    }

    fn signer(&self) -> &Address {
        self.tx.signer()
    }
}

impl<TxEnv: Clone, T> ToTxEnv<TxEnv> for WithTxEnv<TxEnv, T> {
    fn to_tx_env(&self) -> TxEnv {
        self.tx_env.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Address;
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_evm::block::state_changes::balance_increment_state;
    use alloy_primitives::{address, map::HashMap, U256};
    use core::marker::PhantomData;
    use reth_ethereum_primitives::EthPrimitives;
    use revm::{
        database::{CacheDB, EmptyDB},
        state::AccountInfo,
    };

    #[derive(Clone, Debug, Default)]
    struct TestExecutorProvider;

    impl TestExecutorProvider {
        fn executor<DB>(&self, _db: DB) -> TestExecutor<DB>
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

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = CacheDB::<EmptyDB>::default();
        let executor = provider.executor(db);
        let _ = executor.execute(&Default::default());
    }

    fn setup_state_with_account(
        addr: Address,
        balance: u128,
        nonce: u64,
    ) -> State<CacheDB<EmptyDB>> {
        let db = CacheDB::<EmptyDB>::default();
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
        let addr = address!("0x1000000000000000000000000000000000000000");
        let mut state = setup_state_with_account(addr, 100, 1);

        let mut increments = HashMap::default();
        increments.insert(addr, 0);

        let result = balance_increment_state(&increments, &mut state).unwrap();
        assert!(result.is_empty(), "Zero increments should be ignored");
    }

    #[test]
    fn test_balance_increment_state_empty_increments_map() {
        let mut state = State::builder()
            .with_database(CacheDB::<EmptyDB>::default())
            .with_bundle_update()
            .build();

        let increments = HashMap::default();
        let result = balance_increment_state(&increments, &mut state).unwrap();
        assert!(result.is_empty(), "Empty increments map should return empty state");
    }

    #[test]
    fn test_balance_increment_state_multiple_valid_increments() {
        let addr1 = address!("0x1000000000000000000000000000000000000000");
        let addr2 = address!("0x2000000000000000000000000000000000000000");

        let mut state = setup_state_with_account(addr1, 100, 1);

        let account2 =
            AccountInfo { balance: U256::from(200), nonce: 1, code_hash: KECCAK_EMPTY, code: None };
        state.insert_account(addr2, account2);

        let mut increments = HashMap::default();
        increments.insert(addr1, 50);
        increments.insert(addr2, 100);

        let result = balance_increment_state(&increments, &mut state).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(&addr1).unwrap().info.balance, U256::from(100));
        assert_eq!(result.get(&addr2).unwrap().info.balance, U256::from(200));
    }

    #[test]
    fn test_balance_increment_state_mixed_zero_and_nonzero_increments() {
        let addr1 = address!("0x1000000000000000000000000000000000000000");
        let addr2 = address!("0x2000000000000000000000000000000000000000");

        let mut state = setup_state_with_account(addr1, 100, 1);

        let account2 =
            AccountInfo { balance: U256::from(200), nonce: 1, code_hash: KECCAK_EMPTY, code: None };
        state.insert_account(addr2, account2);

        let mut increments = HashMap::default();
        increments.insert(addr1, 0);
        increments.insert(addr2, 100);

        let result = balance_increment_state(&increments, &mut state).unwrap();

        assert_eq!(result.len(), 1, "Only non-zero increments should be included");
        assert!(!result.contains_key(&addr1), "Zero increment account should not be included");
        assert_eq!(result.get(&addr2).unwrap().info.balance, U256::from(200));
    }
}
