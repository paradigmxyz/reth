//! Traits for execution.

use crate::{ConfigureEvm, EvmEnv, TxEnvFor};
#[cfg(feature = "std")]
use alloc::{boxed::Box, rc::Rc};
use alloc::{sync::Arc, vec::Vec};
#[cfg(feature = "std")]
use alloy_consensus::BlockHeader as _;
use alloy_consensus::{
    transaction::{Either, Recovered},
    Header,
};
use alloy_eips::eip2718::WithEncoded;
use alloy_primitives::{Address, Bytes, B256};
#[cfg(feature = "std")]
use core::cell::RefCell;
use core::fmt::Debug;
#[cfg(feature = "std")]
use evm2::evm::{CacheDB, Database, Db, DbErrorCode, DbResult, DynDatabase, StateChangeSource};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, EvmError, InternalBlockExecutionError,
    InvalidTxError,
};
use reth_execution_types::{
    extend_state_and_collect_reverts, state_source_size_hint, BlockExecutionResult, BlockReverts,
    ExecutionState, ExecutionStateAccumulator, HashedPostState,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
#[cfg(feature = "std")]
use reth_primitives_traits::BlockTy;
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
use reth_trie_common::updates::TrieUpdates;

/// Converts a value into the transaction environment expected by an EVM configuration.
pub trait IntoTxEnv<TxEnv> {
    /// Converts this value into the configured transaction environment.
    fn into_tx_env(self) -> TxEnv;
}

impl<TxEnv> IntoTxEnv<TxEnv> for TxEnv {
    fn into_tx_env(self) -> TxEnv {
        self
    }
}

/// Controls how execution produces trie-ready hashed post-state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashedStateMode {
    /// Accumulate final hashed post-state in the returned block execution output.
    OutputOnly,
    /// Stream hashed state updates to the provided hook without accumulating output hashed state.
    StreamOnly,
    /// Accumulate final output hashed state and stream each committed update.
    OutputAndStream,
}

impl HashedStateMode {
    /// Returns true if execution should include hashed state in its output.
    pub const fn output(self) -> bool {
        matches!(self, Self::OutputOnly | Self::OutputAndStream)
    }

    /// Returns true if execution should stream hashed state updates.
    pub const fn stream(self) -> bool {
        matches!(self, Self::StreamOnly | Self::OutputAndStream)
    }
}

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

/// A configured block executor.
pub trait BlockExecutor: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// Transaction environment consumed by this executor.
    type Transaction;
    /// Output returned after a transaction is committed.
    type TransactionOutput: Default;

    /// Returns the underlying EVM.
    fn evm(&self) -> &evm2::Evm<evm2::BaseEvmTypes>;

    /// Returns the underlying EVM mutably.
    fn evm_mut(&mut self) -> &mut evm2::Evm<evm2::BaseEvmTypes>;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes<H>(
        &mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<(), BlockExecutionError>
    where
        H: FnMut(HashedPostState);

    /// Executes a transaction.
    fn execute_transaction<H>(
        &mut self,
        transaction: Self::Transaction,
        on_hashed_state_update: &mut H,
    ) -> Result<Self::TransactionOutput, BlockExecutionError>
    where
        H: FnMut(HashedPostState);

    /// Returns receipts accumulated so far.
    fn receipts(&self) -> &[ReceiptTy<Self::Primitives>];

    /// Finishes block execution and returns the output.
    fn finish<H>(
        self,
        on_hashed_state_update: &mut H,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Self::Primitives>>, BlockExecutionError>
    where
        H: FnMut(HashedPostState);
}

/// A type that creates configured block executors.
pub trait BlockExecutorFactory: Clone + Debug + Send + Sync + Unpin {
    /// The primitive types used by the factory.
    type Primitives: NodePrimitives;
    /// Transaction environment consumed by executors from this factory.
    type Transaction: Clone + Send + Sync + 'static;
    /// EVM environment consumed by this factory.
    type EvmEnv: EvmEnv;
    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;
    /// Block executor returned by this factory.
    type Executor<'a, DB>: BlockExecutor<
        Primitives = Self::Primitives,
        Transaction = Self::Transaction,
        TransactionOutput = GasOutput,
    >
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Creates a configured block executor.
    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: Self::ExecutionCtx<'a>,
        hashed_state_mode: HashedStateMode,
    ) -> Self::Executor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Returns the transaction shape consumed by the configured EVM.
    fn evm_tx<'a>(
        &self,
        tx: &'a Self::Transaction,
    ) -> &'a <evm2::BaseEvmTypes as evm2::EvmTypes>::Tx;
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
            state_provider,
            state_root,
            block_access_list_hash,
        }
    }
}

/// A type that assembles a block from execution output.
pub trait BlockAssembler<F: BlockExecutorFactory>: Clone + Debug + Send + Sync + Unpin {
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
pub trait BlockBuilder {
    /// The primitive types used by the inner [`BlockExecutor`].
    type Primitives: NodePrimitives;
    /// Inner block executor.
    type Executor: BlockExecutor<Primitives = Self::Primitives, TransactionOutput = GasOutput>;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the transaction output, and saves it for block
    /// assembly.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&GasOutput),
    ) -> Result<GasOutput, BlockExecutionError>;

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
    ) -> Result<BlockBuilderOutcome<Self::Primitives>, BlockExecutionError>;

    /// Provides mutable access to the inner [`BlockExecutor`].
    fn executor_mut(&mut self) -> &mut Self::Executor;

    /// Provides access to the inner [`BlockExecutor`].
    fn executor(&self) -> &Self::Executor;

    /// Provides mutable access to the underlying EVM.
    fn evm_mut(&mut self) -> &mut evm2::Evm<evm2::BaseEvmTypes> {
        self.executor_mut().evm_mut()
    }

    /// Provides access to the underlying EVM.
    fn evm(&self) -> &evm2::Evm<evm2::BaseEvmTypes> {
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
    pub assembler: Assembler,
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

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.executor.apply_pre_execution_changes(&mut |_| {})
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&GasOutput),
    ) -> Result<GasOutput, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        let transaction = tx.tx().clone();
        let sender = *tx.signer();
        let output = self.executor.execute_transaction(tx_env, &mut |_| {})?;
        f(&output);
        self.transactions.push(transaction);
        self.senders.push(sender);
        Ok(output)
    }

    fn finish(
        self,
        state_provider: impl StateProvider,
        state_root_precomputed: Option<(B256, TrieUpdates)>,
    ) -> Result<BlockBuilderOutcome<N>, BlockExecutionError> {
        let output = self.executor.finish(&mut |_| {})?;
        let hashed_state = output
            .hashed_state
            .clone()
            .unwrap_or_else(|| state_provider.hashed_post_state(output.state.inner()));
        let (state_root, trie_updates) = match state_root_precomputed {
            Some(precomputed) => precomputed,
            None => state_provider
                .state_root_with_updates(hashed_state.clone())
                .map_err(BlockExecutionError::other)?,
        };

        let block = self.assembler.assemble_block(BlockAssemblerInput {
            evm_env: self.evm_env,
            execution_ctx: self.ctx,
            parent: self.parent,
            transactions: self.transactions,
            output: &output.result,
            state_provider: &state_provider,
            state_root,
            block_access_list_hash: None,
        })?;
        let block = RecoveredBlock::new_unhashed(block, self.senders);

        Ok(BlockBuilderOutcome {
            execution_result: output.result,
            hashed_state,
            trie_updates,
            block,
            block_access_list: None,
        })
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
    type Error: core::error::Error + Send + Sync + 'static;

    /// Executes a single block and returns [`BlockExecutionResult`], without the state changes.
    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Consumes the type and executes the block.
    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Executes multiple inputs in the batch and returns an aggregated [`ExecutionOutcome`].
    fn execute_batch<'a, I>(
        self,
        blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>;

    /// The size hint of the batch's tracked state size.
    fn size_hint(&self) -> usize;

    /// Converts the accumulated batch state and the provided execution results into an execution
    /// outcome.
    fn into_execution_outcome(
        self,
        first_block: u64,
        results: Vec<BlockExecutionResult<ReceiptTy<Self::Primitives>>>,
    ) -> ExecutionOutcome<ReceiptTy<Self::Primitives>>;

    /// Takes the encoded block access list from executor.
    fn take_bal(&mut self) -> Option<Bytes>;
}

/// Generic block executor backed by a [`ConfigureEvm`] implementation.
#[cfg(feature = "std")]
#[expect(missing_debug_implementations)]
pub struct BasicBlockExecutor<Evm, DB: Database> {
    evm_config: Evm,
    database: DB,
    batch_database: SharedBatchDatabase<DB>,
    batch_state: ExecutionStateAccumulator,
    batch_block_states: Vec<ExecutionState>,
    batch_block_reverts: Vec<BlockReverts>,
}

#[cfg(feature = "std")]
impl<Evm, DB> BasicBlockExecutor<Evm, DB>
where
    DB: Database + Clone,
{
    /// Creates a new generic block executor.
    pub fn new(evm_config: Evm, database: DB) -> Self {
        Self {
            evm_config,
            batch_database: SharedBatchDatabase::new(database.clone()),
            database,
            batch_state: ExecutionStateAccumulator::new(),
            batch_block_states: Vec::new(),
            batch_block_reverts: Vec::new(),
        }
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> BasicBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    fn execute_block_with_database(
        &self,
        block: &RecoveredBlock<BlockTy<Evm::Primitives>>,
        database: impl DynDatabase + 'static,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, BlockExecutionError> {
        let evm_env =
            self.evm_config.evm_env(block.header()).map_err(BlockExecutionError::other)?;
        let evm = self.evm_config.evm_with_env(database, evm_env);
        let ctx = self
            .evm_config
            .context_for_block(block.sealed_block())
            .map_err(BlockExecutionError::other)?;
        let mut executor =
            self.evm_config.create_executor::<DB>(evm, ctx, HashedStateMode::OutputOnly);

        executor.apply_pre_execution_changes(&mut |_| {})?;
        for transaction in block.clone_transactions_recovered() {
            let tx_env = TxEnvFor::<Evm>::from(transaction);
            executor.execute_transaction(tx_env, &mut |_| {})?;
        }
        executor.finish(&mut |_| {})
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> Executor for BasicBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = Evm::Primitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let output = self.execute_block_with_database(block, self.batch_database.clone())?;
        self.batch_database.commit_source(&output.state);

        let block_state = output.state.into_inner();
        self.batch_block_reverts
            .push(extend_state_and_collect_reverts(&mut self.batch_state, &block_state));
        self.batch_block_states.push(block_state);

        Ok(output.result)
    }

    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        self.execute_block_with_database(block, Db::new(self.database.clone()))
    }

    fn execute_batch<'a, I>(
        self,
        blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>,
    {
        let mut blocks = blocks.into_iter();
        let Some(block) = blocks.next() else { return Ok(ExecutionOutcome::default()) };

        let first_block = block.header().number();
        let mut executor = self;
        let mut results = Vec::new();

        results.push(executor.execute_one(block)?);

        for block in blocks {
            results.push(executor.execute_one(block)?);
        }

        Ok(executor.into_execution_outcome(first_block, results))
    }

    fn size_hint(&self) -> usize {
        state_source_size_hint(&self.batch_state)
    }

    fn into_execution_outcome(
        self,
        first_block: u64,
        results: Vec<BlockExecutionResult<ReceiptTy<Self::Primitives>>>,
    ) -> ExecutionOutcome<ReceiptTy<Self::Primitives>> {
        ExecutionOutcome::from_aggregated_state(
            first_block,
            self.batch_state,
            self.batch_block_states,
            self.batch_block_reverts,
            results,
        )
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

#[cfg(feature = "std")]
struct SharedBatchDatabase<DB: Database> {
    inner: Rc<RefCell<CacheDB<Db<DB>>>>,
}

#[cfg(feature = "std")]
impl<DB: Database> Clone for SharedBatchDatabase<DB> {
    fn clone(&self) -> Self {
        Self { inner: Rc::clone(&self.inner) }
    }
}

#[cfg(feature = "std")]
impl<DB> SharedBatchDatabase<DB>
where
    DB: Database,
{
    fn new(database: DB) -> Self {
        Self { inner: Rc::new(RefCell::new(CacheDB::new(Db::new(database)))) }
    }

    fn commit_source<S: StateChangeSource>(&self, source: &S) {
        self.inner.borrow_mut().commit_source(source);
    }
}

#[cfg(feature = "std")]
impl<DB> DynDatabase for SharedBatchDatabase<DB>
where
    DB: Database + 'static,
{
    fn get_account(
        &mut self,
        address: &alloy_primitives::Address,
    ) -> DbResult<Option<evm2::evm::AccountInfo>> {
        self.inner.borrow_mut().get_account(address)
    }

    fn get_code_by_hash(
        &mut self,
        code_hash: &alloy_primitives::B256,
    ) -> DbResult<evm2::bytecode::Bytecode> {
        self.inner.borrow_mut().get_code_by_hash(code_hash)
    }

    fn get_storage(
        &mut self,
        address: &alloy_primitives::Address,
        key: &evm2::interpreter::Word,
    ) -> DbResult<evm2::interpreter::Word> {
        self.inner.borrow_mut().get_storage(address, key)
    }

    fn get_block_hash(
        &mut self,
        number: &evm2::interpreter::Word,
    ) -> DbResult<Option<alloy_primitives::B256>> {
        self.inner.borrow_mut().get_block_hash(number)
    }

    fn error(&mut self, code: DbErrorCode) -> Box<dyn core::error::Error> {
        self.inner.borrow_mut().error(code)
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

    fn into_execution_outcome(
        self,
        first_block: u64,
        results: Vec<BlockExecutionResult<ReceiptTy<Self::Primitives>>>,
    ) -> ExecutionOutcome<ReceiptTy<Self::Primitives>> {
        ExecutionOutcome::from_aggregated_state(
            first_block,
            ExecutionStateAccumulator::new(),
            Vec::new(),
            Vec::new(),
            results,
        )
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

/// A helper trait marking a 'static type that can be converted into an [`ExecutableTxParts`] for
/// block executor.
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
