//! Traits for execution.

#[cfg(feature = "std")]
use crate::{database::BorrowedDatabase, Database};
use crate::{ConfigureEvm, DynDatabase, EvmEnv, TxEnvFor};
use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{
    transaction::{Either, Recovered},
    BlockHeader as _, Header,
};
use alloy_eips::eip2718::{Typed2718, WithEncoded};
use alloy_primitives::{Address, Bytes, B256};
use core::fmt::Debug;
#[cfg(feature = "std")]
use evm2::evm::{CacheDB, Db};
use evm2::{Evm, EvmTypes};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, EvmError, InternalBlockExecutionError,
    InvalidTxError,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_execution_types::{
    BlockExecutionResult, ExecutionOutcomeState, ExecutionState, ExecutionStateChangeSink,
    HashedPostState,
};
#[cfg(feature = "std")]
use reth_primitives_traits::BlockTy;
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
use reth_trie_common::updates::TrieUpdates;

/// Gas used by a successfully executed transaction.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GasOutput {
    /// Gas used for receipt accounting.
    tx_gas_used: u64,
    /// Gas used for state-capacity accounting.
    state_gas_used: u64,
}

impl GasOutput {
    /// Creates a new gas output.
    pub const fn new(tx_gas_used: u64, state_gas_used: u64) -> Self {
        Self { tx_gas_used, state_gas_used }
    }

    /// Returns transaction gas used for receipt accounting.
    pub const fn tx_gas_used(&self) -> u64 {
        self.tx_gas_used
    }

    /// Returns state gas used for state-capacity accounting.
    pub const fn state_gas_used(&self) -> u64 {
        self.state_gas_used
    }
}

impl From<u64> for GasOutput {
    fn from(gas_used: u64) -> Self {
        Self::new(gas_used, gas_used)
    }
}

/// Marks whether transaction changes should be committed into block executor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CommitChanges {
    /// Transaction changes should be committed.
    Yes,
    /// Transaction changes should not be committed.
    No,
}

impl CommitChanges {
    /// Returns `true` if transaction changes should be committed.
    pub const fn should_commit(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// A configured block executor.
pub trait BlockExecutor: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// EVM instance used by this executor.
    type Evm;
    /// Transaction environment consumed by this executor.
    type Transaction;
    /// Raw transaction execution result produced before receipt conversion.
    type TransactionResult;
    /// Output returned after a transaction is committed.
    type TransactionOutput: Default;

    /// Returns the underlying EVM.
    fn evm(&self) -> &Self::Evm;

    /// Returns the underlying EVM mutably.
    fn evm_mut(&mut self) -> &mut Self::Evm;

    /// Sets a hook for streamed hashed state updates emitted during block execution.
    ///
    /// Returns `true` if the hook was installed.
    fn set_state_hook(&mut self, _hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        false
    }

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the transaction result, and commits changes when
    /// `f` returns [`CommitChanges::Yes`].
    fn execute_transaction_with_commit_condition(
        &mut self,
        transaction: Self::Transaction,
        f: impl FnOnce(&Self::TransactionResult) -> CommitChanges,
    ) -> Result<Option<Self::TransactionOutput>, BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the transaction result, and commits changes.
    fn execute_transaction_with_result_closure(
        &mut self,
        transaction: Self::Transaction,
        f: impl FnOnce(&Self::TransactionResult),
    ) -> Result<Self::TransactionOutput, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(transaction, |result| {
            f(result);
            CommitChanges::Yes
        })
        .map(Option::unwrap_or_default)
    }

    /// Executes a transaction and commits changes.
    fn execute_transaction(
        &mut self,
        transaction: Self::Transaction,
    ) -> Result<Self::TransactionOutput, BlockExecutionError> {
        self.execute_transaction_with_result_closure(transaction, |_| {})
    }

    /// Returns receipts accumulated so far.
    fn receipts(&self) -> &[ReceiptTy<Self::Primitives>];

    /// Finishes block execution and returns the output.
    fn finish(
        self,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Self::Primitives>>, BlockExecutionError>;
}

/// A type that creates configured block executors.
pub trait BlockExecutorFactory {
    /// The primitive types used by the factory.
    type Primitives: NodePrimitives;
    /// Transaction environment consumed by executors from this factory.
    type Transaction;
    /// EVM instance consumed by executors from this factory.
    type Evm<'a>: ExecuteAndDiscard<Self::Transaction>;
    /// EVM environment consumed by this factory.
    type EvmEnv: EvmEnv;
    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;
    /// Block executor returned by this factory.
    type Executor<'a>: BlockExecutor<
        Primitives = Self::Primitives,
        Evm = Self::Evm<'a>,
        Transaction = Self::Transaction,
        TransactionOutput = GasOutput,
    >
    where
        Self: 'a;

    /// Creates a configured block executor.
    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a;

    /// Creates an EVM instance with the configured execution environment.
    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: DynDatabase + 'a;

    /// Creates an EVM instance with the configured execution environment over a typed database.
    #[cfg(feature = "std")]
    fn evm_with_database<'a, DB>(&self, db: DB, evm_env: Self::EvmEnv) -> Self::Evm<'a>
    where
        DB: Database + 'a,
    {
        self.evm_with_env(Db::new(db), evm_env)
    }
}

/// An EVM instance that can execute a transaction and discard its writes.
pub trait ExecuteAndDiscard<Tx> {
    /// Executes a transaction and discards its writes while streaming observed state changes into
    /// `sink`.
    fn execute_and_discard<S>(
        &mut self,
        transaction: &Tx,
        sink: &mut S,
    ) -> Result<(), BlockExecutionError>
    where
        S: ExecutionStateChangeSink,
        S::Error: Debug;
}

impl<T, Tx> ExecuteAndDiscard<Tx> for Evm<'_, T>
where
    T: EvmTypes<Tx: Typed2718>,
    Tx: AsRef<T::Tx>,
{
    fn execute_and_discard<S>(
        &mut self,
        transaction: &Tx,
        sink: &mut S,
    ) -> Result<(), BlockExecutionError>
    where
        S: ExecutionStateChangeSink,
        S::Error: Debug,
    {
        let executed = self.transact(transaction.as_ref()).map_err(|err| {
            BlockExecutionError::msg(format!("discarded transaction execution failed: {err:?}"))
        })?;

        if let Some(code) = executed.result().error_code {
            let _ = executed.discard();
            return Err(BlockExecutionError::msg(format!(
                "discarded transaction database error: {code:?}"
            )))
        }

        executed.discard_with(sink).map(|_| ()).map_err(|err| {
            BlockExecutionError::msg(format!("discarded state sink failed: {err:?}"))
        })
    }
}

/// Input for block assembly.
#[expect(missing_debug_implementations)]
#[non_exhaustive]
pub struct BlockAssemblerInput<'a, 'b, F: BlockExecutorFactory + 'a, H = Header> {
    /// EVM environment used for block execution.
    pub evm_env: F::EvmEnv,
    /// Execution context used for block execution.
    pub execution_ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<H>,
    /// Transactions included in the block.
    pub transactions: Vec<TxTy<F::Primitives>>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<ReceiptTy<F::Primitives>>,
    /// Execution state after block execution.
    pub execution_state: &'b ExecutionState,
    /// Provider with access to state.
    pub state_provider: &'b dyn StateProvider,
    /// State root for the assembled block.
    pub state_root: B256,
    /// Block access list hash.
    pub block_access_list_hash: Option<B256>,
}

impl<'a, 'b, F: BlockExecutorFactory + 'a, H> BlockAssemblerInput<'a, 'b, F, H> {
    /// Creates a new [`BlockAssemblerInput`].
    #[expect(clippy::too_many_arguments)]
    pub const fn new(
        evm_env: F::EvmEnv,
        execution_ctx: F::ExecutionCtx<'a>,
        parent: &'a SealedHeader<H>,
        transactions: Vec<TxTy<F::Primitives>>,
        output: &'b BlockExecutionResult<ReceiptTy<F::Primitives>>,
        execution_state: &'b ExecutionState,
        state_provider: &'b dyn StateProvider,
        state_root: B256,
        block_access_list_hash: Option<B256>,
    ) -> Self {
        Self {
            evm_env,
            execution_ctx,
            parent,
            transactions,
            output,
            execution_state,
            state_provider,
            state_root,
            block_access_list_hash,
        }
    }
}

/// A type that assembles a block from execution output.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockAssembler<F: BlockExecutorFactory> {
    /// Block produced by the assembler.
    type Block: Block;

    /// Assembles a block from execution output.
    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F, HeaderTy<F::Primitives>>,
    ) -> Result<Self::Block, BlockExecutionError>;
}

/// Output of block building.
#[derive(Debug, Clone)]
pub struct BlockBuilderOutcome<N: NodePrimitives> {
    /// Result of block execution.
    pub execution_result: BlockExecutionResult<N::Receipt>,
    /// Hashed state after execution.
    pub hashed_state: HashedPostState,
    /// Trie updates collected during state-root calculation.
    pub trie_updates: TrieUpdates,
    /// The built block.
    pub block: RecoveredBlock<N::Block>,
    /// Encoded block access list built during execution.
    pub block_access_list: Option<Bytes>,
}

/// A type that knows how to execute transactions and assemble a block.
pub trait BlockBuilder: Sized {
    /// The primitive types used by the inner [`BlockExecutor`].
    type Primitives: NodePrimitives;
    /// Inner block executor.
    type Executor: BlockExecutor<Primitives = Self::Primitives, TransactionOutput = GasOutput>;
    /// EVM environment used for block execution.
    type EvmEnv: EvmEnv;

    /// Returns the EVM environment used for block execution.
    fn evm_env(&self) -> &Self::EvmEnv;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a transaction and saves it for block assembly only if committed.
    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::TransactionResult) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the transaction result, and saves it for block
    /// assembly.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::TransactionResult),
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(tx, |result| {
            f(result);
            CommitChanges::Yes
        })
        .map(Option::unwrap_or_default)
    }

    /// Executes a transaction and saves it for block assembly.
    fn execute_transaction(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_result_closure(tx, |_| ())
    }

    /// Completes block building.
    fn finish(
        self,
        state_provider: impl StateProvider,
        state_root_precomputed: Option<(B256, TrieUpdates)>,
    ) -> Result<BlockBuilderOutcome<Self::Primitives>, BlockExecutionError> {
        self.finish_with_state_root(state_provider, move |_| Ok(state_root_precomputed))
    }

    /// Completes block building, resolving an optional state root after execution is finalized.
    fn finish_with_state_root(
        self,
        state_provider: impl StateProvider,
        state_root: impl FnOnce(
            &BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        ) -> Result<Option<(B256, TrieUpdates)>, BlockExecutionError>,
    ) -> Result<BlockBuilderOutcome<Self::Primitives>, BlockExecutionError>;

    /// Sets a hook for streamed hashed state updates emitted while building a block.
    ///
    /// Returns `true` if the hook was installed.
    fn set_state_hook(&mut self, _hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        false
    }

    /// Provides mutable access to the inner [`BlockExecutor`].
    fn executor_mut(&mut self) -> &mut Self::Executor;

    /// Provides access to the inner [`BlockExecutor`].
    fn executor(&self) -> &Self::Executor;

    /// Provides mutable access to the underlying EVM.
    fn evm_mut(&mut self) -> &mut <Self::Executor as BlockExecutor>::Evm {
        self.executor_mut().evm_mut()
    }

    /// Provides access to the underlying EVM.
    fn evm(&self) -> &<Self::Executor as BlockExecutor>::Evm {
        self.executor().evm()
    }

    /// Consumes the type and returns the underlying executor.
    fn into_executor(self) -> Self::Executor;
}

/// A block builder backed by a configured [`BlockExecutor`].
#[derive(Debug)]
pub struct BasicBlockBuilder<'a, F, Executor, Assembler, N>
where
    F: BlockExecutorFactory + 'a,
    N: NodePrimitives,
{
    /// The block executor used to execute transactions.
    pub executor: Executor,
    /// EVM environment used for execution.
    pub evm_env: F::EvmEnv,
    /// Transactions executed in this block.
    pub transactions: Vec<TxTy<N>>,
    /// Senders for executed transactions.
    pub senders: Vec<Address>,
    /// Block execution context.
    pub ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<HeaderTy<N>>,
    /// Block assembler.
    pub assembler: &'a Assembler,
}

impl<'a, F, Assembler, N> BasicBlockBuilder<'a, F, F::Executor<'a>, Assembler, N>
where
    F: BlockExecutorFactory<Primitives = N> + 'a,
    Assembler: BlockAssembler<F, Block = N::Block> + 'a,
    N: NodePrimitives,
{
    /// Creates a block builder that accumulates final hashed state in the execution output.
    pub fn new(
        executor_factory: &'a F,
        assembler: &'a Assembler,
        evm: F::Evm<'a>,
        evm_env: F::EvmEnv,
        parent: &'a SealedHeader<HeaderTy<N>>,
        ctx: F::ExecutionCtx<'a>,
    ) -> Self {
        Self {
            executor: executor_factory.create_executor(evm, ctx.clone()),
            evm_env,
            transactions: Vec::new(),
            senders: Vec::new(),
            ctx,
            parent,
            assembler,
        }
    }
}

/// Conversions for executable transactions consumed by a block executor.
pub trait ExecutorTx<Executor: BlockExecutor> {
    /// Recovered transaction accessor type.
    type Recovered: RecoveredTx<TxTy<Executor::Primitives>>;

    /// Converts the transaction into executor transaction input and recovered accessor.
    fn into_parts(self) -> (Executor::Transaction, Self::Recovered);
}

impl<T, Executor> ExecutorTx<Executor> for T
where
    Executor: BlockExecutor,
    T: ExecutableTxParts<Executor::Transaction, TxTy<Executor::Primitives>>,
{
    type Recovered = T::Recovered;

    fn into_parts(self) -> (Executor::Transaction, Self::Recovered) {
        ExecutableTxParts::into_parts(self)
    }
}

impl<'a, F, Executor, Assembler, N> BlockBuilder
    for BasicBlockBuilder<'a, F, Executor, Assembler, N>
where
    F: BlockExecutorFactory<Primitives = N>,
    Executor:
        BlockExecutor<Primitives = N, Transaction = F::Transaction, TransactionOutput = GasOutput>,
    Assembler: BlockAssembler<F, Block = N::Block>,
    N: NodePrimitives,
    TxTy<N>: Clone,
{
    type Primitives = N;
    type Executor = Executor;
    type EvmEnv = F::EvmEnv;

    fn evm_env(&self) -> &Self::EvmEnv {
        &self.evm_env
    }

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.executor.apply_pre_execution_changes()
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::TransactionResult) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        let transaction = tx.tx().clone();
        let sender = *tx.signer();
        if let Some(output) = self.executor.execute_transaction_with_commit_condition(tx_env, f)? {
            self.transactions.push(transaction);
            self.senders.push(sender);
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }

    fn finish_with_state_root(
        self,
        state_provider: impl StateProvider,
        state_root: impl FnOnce(
            &BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        ) -> Result<Option<(B256, TrieUpdates)>, BlockExecutionError>,
    ) -> Result<BlockBuilderOutcome<N>, BlockExecutionError> {
        let Self { executor, evm_env, transactions, senders, ctx, parent, assembler } = self;

        let output = executor.finish()?;
        let hashed_state = output
            .hashed_state
            .clone()
            .unwrap_or_else(|| state_provider.hashed_post_state(output.state.inner()));
        let (state_root, trie_updates) = match state_root(&output)? {
            Some(precomputed) => precomputed,
            None => state_provider
                .state_root_with_updates(hashed_state.clone())
                .map_err(BlockExecutionError::other)?,
        };

        let block = assembler.assemble_block(BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            parent,
            transactions,
            output: &output.result,
            execution_state: output.state.inner(),
            state_provider: &state_provider,
            state_root,
            block_access_list_hash: None,
        })?;
        let block = RecoveredBlock::new_unhashed(block, senders);

        Ok(BlockBuilderOutcome {
            execution_result: output.result,
            hashed_state,
            trie_updates,
            block,
            block_access_list: None,
        })
    }

    fn set_state_hook(&mut self, hook: impl FnMut(HashedPostState) + Send + 'static) -> bool {
        self.executor.set_state_hook(hook)
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

/// A type that knows how to execute blocks.
pub trait Executor: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// The error type returned by the executor.
    type Error;

    /// Executes a single block and returns [`BlockExecutionResult`], without the state changes.
    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Executes a single block and streams hashed state updates to the provided hook.
    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static;

    /// Consumes the type and executes the block.
    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Consumes the type, executes the block, and streams hashed state updates to the provided
    /// hook.
    fn execute_with_state_hook<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static;

    /// Executes multiple inputs in the batch and returns an aggregated [`ExecutionOutcome`].
    fn execute_batch<'a, I>(
        mut self,
        blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>,
    {
        let blocks_iter = blocks.into_iter();
        let capacity = blocks_iter.size_hint().0;
        let mut results = Vec::with_capacity(capacity);
        let mut first_block = None;
        for block in blocks_iter {
            if first_block.is_none() {
                first_block = Some(block.header().number());
            }
            results.push(self.execute_one(block)?);
        }

        Ok(ExecutionOutcome::from_blocks(
            first_block.unwrap_or_default(),
            self.into_state(),
            results,
        ))
    }

    /// The size hint of the batch's tracked state size.
    fn size_hint(&self) -> usize;

    /// Converts the executor into its accumulated batch state.
    fn into_state(self) -> ExecutionOutcomeState;

    /// Takes the encoded block access list from executor.
    fn take_bal(&mut self) -> Option<Bytes>;
}

/// Generic block executor backed by a [`ConfigureEvm`] implementation.
#[cfg(feature = "std")]
#[expect(missing_debug_implementations)]
pub struct BasicBlockExecutor<Evm, DB: Database> {
    evm_config: Evm,
    batch_database: CacheDB<Db<DB>>,
    batch_state: ExecutionOutcomeState,
}

#[cfg(feature = "std")]
impl<Evm, DB: Database> BasicBlockExecutor<Evm, DB> {
    /// Creates a new generic block executor.
    pub fn new(evm_config: Evm, database: DB) -> Self {
        Self {
            evm_config,
            batch_database: CacheDB::new(Db::new(database)),
            batch_state: ExecutionOutcomeState::default(),
        }
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> BasicBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database,
{
    fn execute_block_with_database(
        evm_config: &Evm,
        block: &RecoveredBlock<BlockTy<Evm::Primitives>>,
        database: impl DynDatabase,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, BlockExecutionError> {
        Self::execute_block_with_database_and_state_hook(evm_config, block, database, None)
    }

    fn execute_block_with_database_and_state_hook(
        evm_config: &Evm,
        block: &RecoveredBlock<BlockTy<Evm::Primitives>>,
        database: impl DynDatabase,
        state_hook: Option<Box<dyn FnMut(HashedPostState) + Send>>,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, BlockExecutionError> {
        let evm_env = evm_config.evm_env(block.header()).map_err(BlockExecutionError::other)?;
        let evm = evm_config.block_executor_factory().evm_with_env(database, evm_env);
        let ctx = evm_config
            .context_for_block(block.sealed_block())
            .map_err(BlockExecutionError::other)?;
        let mut executor = evm_config.block_executor_factory().create_executor(evm, ctx);
        if let Some(hook) = state_hook &&
            !executor.set_state_hook(hook)
        {
            return Err(BlockExecutionError::msg("block executor does not support state hooks"))
        }

        executor.apply_pre_execution_changes()?;
        for transaction in block.clone_transactions_recovered() {
            let tx_env = evm_config.tx_env(transaction);
            executor.execute_transaction(tx_env)?;
        }
        executor.finish()
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> Executor for BasicBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database,
{
    type Primitives = Evm::Primitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let output = Self::execute_block_with_database(
            &self.evm_config,
            block,
            BorrowedDatabase::new(&mut self.batch_database),
        )?;
        self.batch_database.commit_source(&output.state);

        let block_state = output.state.into_inner();
        self.batch_state.push_block_state(block_state);

        Ok(output.result)
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static,
    {
        let output = Self::execute_block_with_database_and_state_hook(
            &self.evm_config,
            block,
            BorrowedDatabase::new(&mut self.batch_database),
            Some(Box::new(state_hook)),
        )?;
        self.batch_database.commit_source(&output.state);

        let block_state = output.state.into_inner();
        self.batch_state.push_block_state(block_state);

        Ok(output.result)
    }

    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Self::execute_block_with_database(&self.evm_config, block, self.batch_database)
    }

    fn execute_with_state_hook<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static,
    {
        Self::execute_block_with_database_and_state_hook(
            &self.evm_config,
            block,
            self.batch_database,
            Some(Box::new(state_hook)),
        )
    }

    fn size_hint(&self) -> usize {
        self.batch_state.size_hint()
    }

    fn into_state(self) -> ExecutionOutcomeState {
        self.batch_state
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

/// Executor returned by configurations that do not support block execution in the active build.
#[expect(missing_debug_implementations)]
pub struct UnsupportedExecutor<N> {
    _marker: core::marker::PhantomData<N>,
}

impl<N> Default for UnsupportedExecutor<N> {
    fn default() -> Self {
        Self { _marker: core::marker::PhantomData }
    }
}

impl<N: NodePrimitives> Executor for UnsupportedExecutor<N> {
    type Primitives = N;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn execute(
        self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static,
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn execute_with_state_hook<F>(
        self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(HashedPostState) + Send + 'static,
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn execute_batch<'a, I>(
        self,
        _blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>,
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn size_hint(&self) -> usize {
        0
    }

    fn into_state(self) -> ExecutionOutcomeState {
        ExecutionOutcomeState::default()
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

/// Helper trait to abstract over recovered transaction wrappers.
#[auto_impl::auto_impl(&)]
pub trait RecoveredTx<T> {
    /// Returns the transaction.
    fn tx(&self) -> &T;

    /// Returns the signer of the transaction.
    fn signer(&self) -> &Address;
}

impl<T> RecoveredTx<T> for Recovered<&T> {
    fn tx(&self) -> &T {
        self.inner()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<T> RecoveredTx<T> for Recovered<Arc<T>> {
    fn tx(&self) -> &T {
        self.inner().as_ref()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<T> RecoveredTx<T> for Recovered<T> {
    fn tx(&self) -> &T {
        self.inner()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithEncoded<T> {
    fn tx(&self) -> &Tx {
        self.1.tx()
    }

    fn signer(&self) -> &Address {
        self.1.signer()
    }
}

impl<L, R, Tx> RecoveredTx<Tx> for Either<L, R>
where
    L: RecoveredTx<Tx>,
    R: RecoveredTx<Tx>,
{
    fn tx(&self) -> &Tx {
        match self {
            Self::Left(l) => l.tx(),
            Self::Right(r) => r.tx(),
        }
    }

    fn signer(&self) -> &Address {
        match self {
            Self::Left(l) => l.signer(),
            Self::Right(r) => r.signer(),
        }
    }
}

impl<Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for Arc<T> {
    fn tx(&self) -> &Tx {
        (**self).tx()
    }

    fn signer(&self) -> &Address {
        (**self).signer()
    }
}

/// Helper trait to split an executable transaction into an EVM transaction environment and its
/// recovered transaction accessor.
pub trait ExecutableTxParts<TxEnv, T> {
    /// The recovered transaction accessor type.
    type Recovered: RecoveredTx<T>;

    /// Converts the transaction into the executable transaction environment and recovered
    /// transaction accessor.
    fn into_parts(self) -> (TxEnv, Self::Recovered);
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for Recovered<T>
where
    TxEnv: From<Self>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.clone().into(), self)
    }
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for WithEncoded<Recovered<T>>
where
    TxEnv: From<Recovered<T>>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.1.clone().into(), self)
    }
}

impl<L, R, TxEnv, T> ExecutableTxParts<TxEnv, T> for Either<L, R>
where
    L: ExecutableTxParts<TxEnv, T>,
    R: ExecutableTxParts<TxEnv, T>,
{
    type Recovered = Either<L::Recovered, R::Recovered>;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        match self {
            Self::Left(l) => {
                let (tx_env, recovered) = l.into_parts();
                (tx_env, Either::Left(recovered))
            }
            Self::Right(r) => {
                let (tx_env, recovered) = r.into_parts();
                (tx_env, Either::Right(recovered))
            }
        }
    }
}

/// A helper trait marking a type that can be converted into an [`ExecutableTxParts`] for block
/// executor.
pub trait ExecutableTxFor<Evm: ConfigureEvm>:
    ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

impl<T, Evm: ConfigureEvm> ExecutableTxFor<Evm> for T where
    T: ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

/// A transaction stored together with its `TxEnv`.
///
/// See also [`ExecutableTxParts`] for types that can be split into a transaction environment and
/// recovered transaction.
#[derive(Debug)]
pub struct WithTxEnv<TxEnv, T> {
    /// The transaction environment for EVM.
    pub tx_env: TxEnv,
    /// The recovered transaction.
    pub tx: Arc<T>,
}

impl<TxEnv, T> WithTxEnv<TxEnv, T> {
    /// Creates a transaction/environment pair from a type that can be split with
    /// [`ExecutableTxParts::into_parts`].
    pub fn new<Tx, InnerTx>(tx: Tx) -> Self
    where
        Tx: ExecutableTxParts<TxEnv, InnerTx, Recovered = T>,
    {
        let (tx_env, tx) = tx.into_parts();
        Self { tx_env, tx: Arc::new(tx) }
    }
}

impl<TxEnv: Clone, T> Clone for WithTxEnv<TxEnv, T> {
    fn clone(&self) -> Self {
        Self { tx_env: self.tx_env.clone(), tx: self.tx.clone() }
    }
}

impl<TxEnv, Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithTxEnv<TxEnv, T> {
    fn tx(&self) -> &Tx {
        self.tx.tx()
    }

    fn signer(&self) -> &Address {
        self.tx.signer()
    }
}

impl<TxEnv, T: RecoveredTx<Tx>, Tx> ExecutableTxParts<TxEnv, Tx> for WithTxEnv<TxEnv, T> {
    type Recovered = Arc<T>;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        (self.tx_env, self.tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_primitives::EthPrimitives;

    #[test]
    fn unsupported_executor_returns_error() {
        let executor = UnsupportedExecutor::<EthPrimitives>::default();
        let err = executor.execute(&Default::default()).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }
}
