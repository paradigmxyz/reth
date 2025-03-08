//! Traits for execution.

use crate::{
    ConfigureEvmFor, Database, EvmEnvFor, EvmFor, HaltReasonFor, InspectorFor, OnStateHook,
};
use alloc::{boxed::Box, vec::Vec};
use alloy_consensus::BlockHeader;
pub use alloy_evm::block::BlockExecutor;
use alloy_evm::{
    block::{BlockExecutorFactory, BlockExecutorFor},
    Evm,
};
use alloy_primitives::{
    map::{DefaultHashBuilder, HashMap},
    Address, B256,
};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::BlockExecutionResult;
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_primitives_traits::{
    BlockTy, HeaderTy, NodePrimitives, ReceiptTy, Recovered, RecoveredBlock, SealedBlock,
    SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
pub use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::{
    context::result::ExecutionResult,
    state::{Account, AccountStatus, EvmState},
};
use revm_database::{states::bundle_state::BundleRetention, BundleState, State};

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
pub struct ExecuteOutput<R> {
    /// Receipts obtained after executing a block.
    pub receipts: Vec<R>,
    /// Cumulative gas used in the block execution.
    pub gas_used: u64,
}

/// A factory that can create block execution strategies.
///
/// This trait extends [`crate::ConfigureEvm`] and provides a way to construct a
/// [`BlockExecutor`]. Strategy is expected to derive most of the context for block
/// execution from the EVM (which includes [`revm::context::BlockEnv`]), and any additional context
/// should be contained in configured [`ExecutionCtx`].
///
/// Strategy is required to provide a way to obtain [`ExecutionCtx`] from either a complete
/// [`SealedBlock`] (in case of execution of an externally obtained block), or from a parent header
/// along with [`crate::ConfigureEvmEnv::NextBlockEnvCtx`] (in the case of block building).
///
/// For more context on the strategy design, see the documentation for [`BlockExecutor`].
///
/// Additionally, trait implementations are expected to define a [`BlockAssembler`] type that is
/// used to assemble blocks. Assembler combined with strategy are used to create a [`BlockBuilder`].
/// [`BlockBuilder`] exposes a simple API for building blocks and can be consumed by payload
/// builder.
///
/// [`ExecutionCtx`]: BlockExecutorFactory::ExecutionCtx
pub trait BlockExecutionStrategyFactory: ConfigureEvmFor<Self::Primitives> + 'static {
    /// Primitive types used by the strategy.
    type Primitives: NodePrimitives;

    /// Block executor factory.
    type BlockExecutorFactory: BlockExecutorFactory<
        EvmFactory = Self::EvmFactory,
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
    >;

    /// A type that knows how to build a block.
    type BlockAssembler: BlockAssembler<Self>;

    /// Provides reference to configured [`BlockExecutorFactory`].
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory;

    /// Provides reference to configured [`BlockAssembler`].
    fn block_assembler(&self) -> &Self::BlockAssembler;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for a given block.
    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for `parent + 1`
    /// block.
    fn context_for_next_block(
        &self,
        parent: &SealedHeader<<Self::Primitives as NodePrimitives>::BlockHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'_>;

    /// Creates a strategy with given EVM and execution context.
    fn create_strategy<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        ctx: <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self::BlockExecutorFactory, DB, I>
    where
        DB: Database,
        I: InspectorFor<&'a mut State<DB>, Self> + 'a,
    {
        self.block_executor_factory().create_executor(evm, ctx)
    }

    /// Creates a strategy for execution of a given block.
    fn strategy_for_block<'a, DB: Database>(
        &'a self,
        db: &'a mut State<DB>,
        block: &'a SealedBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> impl BlockExecutorFor<'a, Self::BlockExecutorFactory, DB> {
        let evm = self.evm_for_block(db, block.header());
        let ctx = self.context_for_block(block);
        self.create_strategy(evm, ctx)
    }

    /// Creates a [`BlockBuilder`]. Should be used when building a new block.
    ///
    /// Block builder wraps an inner [`BlockExecutor`] and has a similar interface. Builder
    /// collects all of the executed transactions, and once [`BlockBuilder::finish`] is called, it
    /// invokes the configured [`BlockAssembler`] to create a block.
    fn create_block_builder<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        ctx: <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    ) -> impl BlockBuilder<
        Primitives = Self::Primitives,
        Executor: BlockExecutorFor<'a, Self::BlockExecutorFactory, DB, I>,
    >
    where
        DB: Database,
        I: InspectorFor<&'a mut State<DB>, Self> + 'a,
    {
        BasicBlockBuilder {
            executor: self.create_strategy(evm, ctx.clone()),
            ctx,
            assembler: self.block_assembler(),
            parent,
            transactions: Vec::new(),
        }
    }

    /// Creates a [`BlockBuilder`] for building of a new block. This is a helper to invoke
    /// [`BlockExecutionStrategyFactory::create_block_builder`].
    fn builder_for_next_block<'a, DB: Database>(
        &'a self,
        db: &'a mut State<DB>,
        parent: &'a SealedHeader<<Self::Primitives as NodePrimitives>::BlockHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<impl BlockBuilder<Primitives = Self::Primitives>, Self::Error> {
        let evm_env = self.next_evm_env(parent, &attributes)?;
        let evm = self.evm_with_env(db, evm_env);
        let ctx = self.context_for_next_block(parent, attributes);
        Ok(self.create_block_builder(evm, parent, ctx))
    }
}

/// Input for block building. Consumed by [`BlockAssembler`].
#[derive(derive_more::Debug)]
#[non_exhaustive]
pub struct BlockAssemblerInput<'a, 'b, Evm: BlockExecutionStrategyFactory> {
    /// Configuration of EVM used when executing the block.
    ///
    /// Contains context relevant to EVM such as [`revm::context::BlockEnv`].
    pub evm_env: EvmEnvFor<Evm>,
    /// [`BlockExecutorFactory::ExecutionCtx`] used to execute the block.
    pub execution_ctx: <Evm::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<HeaderTy<Evm::Primitives>>,
    /// Transactions that were executed in this block.
    pub transactions: Vec<TxTy<Evm::Primitives>>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<ReceiptTy<Evm::Primitives>>,
    /// [`BundleState`] after the block execution.
    pub bundle_state: &'a BundleState,
    /// Provider with access to state.
    #[debug(skip)]
    pub state_provider: &'b dyn StateProvider,
    /// State root for this block.
    pub state_root: B256,
}

/// A type that knows how to assemble a block.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockAssembler<Evm: BlockExecutionStrategyFactory> {
    /// Builds a block. see [`BlockAssemblerInput`] documentation for more details.
    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, Evm>,
    ) -> Result<BlockTy<Evm::Primitives>, BlockExecutionError>;
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

    /// Invokes [`BlockExecutor::execute_transaction_with_result_closure`] and saves the
    /// transaction in internal state.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: Recovered<TxTy<Self::Primitives>>,
        f: impl FnOnce(&ExecutionResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::HaltReason>),
    ) -> Result<u64, BlockExecutionError>;

    /// Invokes [`BlockExecutor::execute_transaction`] and saves the transaction in
    /// internal state.
    fn execute_transaction(
        &mut self,
        tx: Recovered<TxTy<Self::Primitives>>,
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

    /// Helper to access inner [`BlockExecutor::Evm`].
    fn evm_mut(&mut self) -> &mut <Self::Executor as BlockExecutor>::Evm {
        self.executor_mut().evm_mut()
    }

    /// Consumes the type and returns the underlying [`BlockExecutor`].
    fn into_executor(self) -> Self::Executor;
}

struct BasicBlockBuilder<'a, Evm, Executor, Builder>
where
    Evm: BlockExecutionStrategyFactory,
{
    executor: Executor,
    transactions: Vec<Recovered<TxTy<Evm::Primitives>>>,
    ctx: <Evm::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    parent: &'a SealedHeader<HeaderTy<Evm::Primitives>>,
    assembler: Builder,
}

impl<'a, E, DB, Executor, Builder> BlockBuilder for BasicBlockBuilder<'a, E, Executor, Builder>
where
    E: BlockExecutionStrategyFactory,
    Executor: BlockExecutor<
        Receipt = ReceiptTy<E::Primitives>,
        Transaction = TxTy<E::Primitives>,
        Evm: Evm<Spec = E::Spec, HaltReason = HaltReasonFor<E>, DB = &'a mut State<DB>>,
    >,
    DB: Database + 'a,
    Builder: BlockAssembler<E>,
{
    type Primitives = E::Primitives;
    type Executor = Executor;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.executor.apply_pre_execution_changes()
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: Recovered<TxTy<Self::Primitives>>,
        f: impl FnOnce(&ExecutionResult<HaltReasonFor<E>>),
    ) -> Result<u64, BlockExecutionError> {
        let gas_used =
            self.executor.execute_transaction_with_result_closure(tx.as_recovered_ref(), f)?;
        self.transactions.push(tx);
        Ok(gas_used)
    }

    fn finish(
        self,
        state: impl StateProvider,
    ) -> Result<BlockBuilderOutcome<E::Primitives>, BlockExecutionError> {
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

    fn into_executor(self) -> Self::Executor {
        self.executor
    }
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
    F: BlockExecutionStrategyFactory + 'static,
{
    type Primitives = F::Primitives;

    type Executor<DB: Database> = BasicBlockExecutor<F, DB>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database,
    {
        BasicBlockExecutor::new(self.strategy_factory.clone(), db)
    }
}

/// A generic block executor that uses a [`BlockExecutor`] to
/// execute blocks.
#[allow(missing_debug_implementations, dead_code)]
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
    F: BlockExecutionStrategyFactory,
    DB: Database,
{
    type Primitives = F::Primitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let mut strategy = self.strategy_factory.strategy_for_block(&mut self.db, block);

        strategy.apply_pre_execution_changes()?;
        for tx in block.transactions_recovered() {
            strategy.execute_transaction(tx)?;
        }
        let result = strategy.apply_post_execution_changes()?;

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
        let mut strategy = self
            .strategy_factory
            .strategy_for_block(&mut self.db, block)
            .with_state_hook(Some(Box::new(state_hook)));

        strategy.apply_pre_execution_changes()?;
        for tx in block.transactions_recovered() {
            strategy.execute_transaction(tx)?;
        }
        let result = strategy.apply_post_execution_changes()?;

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
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_primitives::{address, U256};
    use core::marker::PhantomData;
    use reth_ethereum_primitives::EthPrimitives;
    use revm::state::AccountInfo;
    use revm_database::{CacheDB, EmptyDB};

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

        let mut increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
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

        let increments = HashMap::<Address, u128, DefaultHashBuilder>::default();
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
        let addr1 = address!("0x1000000000000000000000000000000000000000");
        let addr2 = address!("0x2000000000000000000000000000000000000000");

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
