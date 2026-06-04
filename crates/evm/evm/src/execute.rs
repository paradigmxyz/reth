//! Traits for execution.

use crate::{ConfigureEvm, Database, OnStateHook, TxEnvFor};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_consensus::{BlockHeader, Header};
use alloy_eips::eip2718::WithEncoded;
pub use alloy_evm::block::{BlockExecutor, BlockExecutorFactory, GasOutput};
use alloy_evm::{
    block::CommitChanges, Evm, EvmEnv, EvmFactory, FromRecoveredTx, FromTxWithEncoded, ToTxEnv,
};
use alloy_primitives::{map::AddressMap, Address, Bytes, B256};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Tracked},
};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_execution_types::{
    BlockExecutionResult, Evm2BlockReverts, Evm2BundleState, Evm2StorageChangeSet,
    Evm2StorageReverts,
};
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, Recovered, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
pub use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::{
    context_interface::Block as _,
    database::{
        states::{bundle_state::BundleRetention, reverts::AccountInfoRevert, RevertToSlot},
        BundleState, State,
    },
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
        Ok(BlockExecutionOutput {
            state: revm_bundle_to_evm2(state.take_bundle(), block.header().number()),
            result,
        })
    }

    /// Executes multiple inputs in the batch, and returns an aggregated [`ExecutionOutcome`].
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
            revm_bundle_to_evm2(self.into_state().take_bundle(), first_block.unwrap_or_default()),
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
        Ok(BlockExecutionOutput {
            state: revm_bundle_to_evm2(state.take_bundle(), block.header().number()),
            result,
        })
    }

    /// Executes the EVM with the given input and accepts a state closure that is always invoked
    /// with the EVM state after execution, even after failure.
    fn execute_with_state_closure_always<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        mut f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        let result = self.execute_one(block);
        let mut state = self.into_state();
        f(&state);

        Ok(BlockExecutionOutput {
            state: revm_bundle_to_evm2(state.take_bundle(), block.header().number()),
            result: result?,
        })
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
        Ok(BlockExecutionOutput {
            state: revm_bundle_to_evm2(state.take_bundle(), block.header().number()),
            result,
        })
    }

    /// Consumes the executor and returns the [`State`] containing all state changes.
    fn into_state(self) -> State<DB>;

    /// The size hint of the batch's tracked state size.
    ///
    /// This is used to optimize DB commits depending on the size of the state.
    fn size_hint(&self) -> usize;

    /// Takes the encoded block access list from executor.
    fn take_bal(&mut self) -> Option<Bytes>;
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
/// - `block_access_list_hash`: Block access list hash (EIP-7928, Amsterdam)
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
///     block_access_list_hash: Some(calculated_bal_hash),
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
    pub evm_env:
        EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    /// [`BlockExecutorFactory::ExecutionCtx`] used to execute the block.
    pub execution_ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<H>,
    /// Transactions that were executed in this block.
    pub transactions: Vec<F::Transaction>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<F::Receipt>,
    /// Bundle state after the block execution.
    pub bundle_state: &'b Evm2BundleState,
    /// Provider with access to state.
    #[debug(skip)]
    pub state_provider: &'b dyn StateProvider,
    /// State root for this block.
    pub state_root: B256,
    /// Block access list hash (EIP-7928, Amsterdam).
    pub block_access_list_hash: Option<B256>,
}

impl<'a, 'b, F: BlockExecutorFactory, H> BlockAssemblerInput<'a, 'b, F, H> {
    /// Creates a new [`BlockAssemblerInput`].
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        evm_env: EvmEnv<
            <F::EvmFactory as EvmFactory>::Spec,
            <F::EvmFactory as EvmFactory>::BlockEnv,
        >,
        execution_ctx: F::ExecutionCtx<'a>,
        parent: &'a SealedHeader<H>,
        transactions: Vec<F::Transaction>,
        output: &'b BlockExecutionResult<F::Receipt>,
        bundle_state: &'b Evm2BundleState,
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
            bundle_state,
            state_provider,
            state_root,
            block_access_list_hash,
        }
    }
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
    /// Encoded block access list built during execution (EIP-7928, Amsterdam).
    ///
    /// This is always `None` in the active evm2 pre-Amsterdam path.
    pub block_access_list: Option<Bytes>,
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
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError>;

    /// Invokes [`BlockExecutor::execute_transaction_with_result_closure`] and saves the
    /// transaction in internal state.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::Result),
    ) -> Result<GasOutput, BlockExecutionError> {
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
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_result_closure(tx, |_| ())
    }

    /// Completes the block building process and returns the [`BlockBuilderOutcome`].
    ///
    /// When `state_root_precomputed` is `None`, the state root is computed internally via
    /// `state_root_with_updates()`. When `Some`, the provided root and trie updates are used
    /// directly, skipping the expensive computation (e.g. when using the sparse trie pipeline).
    fn finish(
        self,
        state_provider: impl StateProvider,
        state_root_precomputed: Option<(B256, TrieUpdates)>,
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

    /// Returns the configured block gas limit.
    fn block_gas_limit(&self) -> u64 {
        self.evm().block().gas_limit()
    }

    /// Returns the configured base fee.
    fn block_base_fee(&self) -> u64 {
        self.evm().block().basefee()
    }

    /// Returns the configured blob gas price, if enabled for the block.
    fn block_blob_gasprice(&self) -> Option<u128> {
        self.evm().block().blob_gasprice()
    }

    /// Returns the configured per-transaction gas limit cap override.
    fn tx_gas_limit_cap(&self) -> Option<u64> {
        self.evm().cfg_env().tx_gas_limit_cap
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
    /// Converts the transaction into a tuple of [`TxEnvFor`] and [`Recovered`].
    fn into_parts(self) -> (<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>);
}

/// Converts a prepared executable transaction wrapper back into its recovered transaction.
pub trait IntoRecoveredTx<InnerTx> {
    /// Recovered transaction type.
    type Recovered;

    /// Converts this wrapper into the recovered transaction it contains.
    fn into_recovered_tx(self) -> Self::Recovered;
}

impl<Executor: BlockExecutor> ExecutorTx<Executor>
    for WithEncoded<Recovered<Executor::Transaction>>
{
    fn into_parts(self) -> (<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>) {
        (self.to_tx_env(), self.1)
    }
}

impl<Executor: BlockExecutor> ExecutorTx<Executor> for Recovered<Executor::Transaction> {
    fn into_parts(self) -> (<Executor::Evm as Evm>::Tx, Self) {
        (self.to_tx_env(), self)
    }
}

impl<Executor: BlockExecutor> ExecutorTx<Executor>
    for (<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>)
{
    fn into_parts(self) -> (<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>) {
        self
    }
}

impl<Executor> ExecutorTx<Executor>
    for WithTxEnv<<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>>
where
    Executor: BlockExecutor<Transaction: Clone>,
{
    fn into_parts(self) -> (<Executor::Evm as Evm>::Tx, Recovered<Executor::Transaction>) {
        (self.tx_env, Arc::unwrap_or_clone(self.tx))
    }
}

impl<T> IntoRecoveredTx<T> for Recovered<T> {
    type Recovered = Self;

    fn into_recovered_tx(self) -> Self::Recovered {
        self
    }
}

impl<T> IntoRecoveredTx<T> for WithEncoded<Recovered<T>> {
    type Recovered = Recovered<T>;

    fn into_recovered_tx(self) -> Self::Recovered {
        self.1
    }
}

impl<TxEnv, T, InnerTx> IntoRecoveredTx<InnerTx> for WithTxEnv<TxEnv, T>
where
    T: IntoRecoveredTx<InnerTx> + Clone,
{
    type Recovered = T::Recovered;

    fn into_recovered_tx(self) -> Self::Recovered {
        Arc::unwrap_or_clone(self.tx).into_recovered_tx()
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
            BlockEnv = <F::EvmFactory as EvmFactory>::BlockEnv,
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
        self.executor.apply_pre_execution_changes().map_err(BlockExecutionError::other)?;
        // BAL execution is Amsterdam-only and remains stubbed for the evm2 pre-Amsterdam path.
        // self.executor.evm_mut().db_mut().bump_bal_index();

        Ok(())
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        if let Some(gas_used) = self
            .executor
            .execute_transaction_with_commit_condition((tx_env, &tx), f)
            .map_err(BlockExecutionError::other)?
        {
            self.transactions.push(tx);
            // BAL execution is Amsterdam-only and remains stubbed for the evm2 pre-Amsterdam path.
            // self.executor.evm_mut().db_mut().bump_bal_index();
            Ok(Some(gas_used))
        } else {
            Ok(None)
        }
    }

    fn finish(
        self,
        state: impl StateProvider,
        state_root_precomputed: Option<(B256, TrieUpdates)>,
    ) -> Result<BlockBuilderOutcome<N>, BlockExecutionError> {
        let (evm, result) = self.executor.finish().map_err(BlockExecutionError::other)?;
        let (db, evm_env) = evm.finish();

        // merge all transitions into bundle state
        db.merge_transitions(BundleRetention::Reverts);
        let block_access_list = None;
        let block_access_list_hash = None;
        // BAL execution is Amsterdam-only and remains stubbed for the evm2 pre-Amsterdam path.
        // let block_access_list = db.take_built_alloy_bal();
        // let block_access_list_hash =
        //     block_access_list.as_ref().map(|bal| compute_block_access_list_hash(bal.as_slice()));
        let bundle_state = revm_bundle_to_evm2(db.bundle_state.clone(), self.parent.number() + 1);

        let hashed_state = state.hashed_post_state(&bundle_state);
        let (state_root, trie_updates) = match state_root_precomputed {
            Some(precomputed) => precomputed,
            None => state
                .state_root_with_updates(hashed_state.clone())
                .map_err(BlockExecutionError::other)?,
        };

        let (transactions, senders) =
            self.transactions.into_iter().map(|tx| tx.into_parts()).unzip();

        let result = alloy_block_execution_result_to_reth(result);
        let block = self.assembler.assemble_block(BlockAssemblerInput {
            evm_env,
            execution_ctx: self.ctx,
            parent: self.parent,
            transactions,
            output: &result,
            bundle_state: &bundle_state,
            state_provider: &state,
            state_root,
            block_access_list_hash,
        })?;

        let block = RecoveredBlock::new_unhashed(block, senders);

        Ok(BlockBuilderOutcome {
            execution_result: result,
            hashed_state,
            trie_updates,
            block,
            block_access_list,
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
        let db = State::builder().with_database(db).with_bundle_update().build();
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
        if block.header().block_access_list_hash().is_some() {
            return Err(BlockValidationError::msg(
                "block access lists are unsupported by the evm2 execution path",
            )
            .into())
        }

        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;

        // BAL execution is Amsterdam-only and remains stubbed for the evm2 pre-Amsterdam path.
        // let has_bal = block.header().block_access_list_hash().is_some();
        //
        // if has_bal {
        //     executor.evm_mut().db_mut().bal_state.bal_builder = Some(Bal::new());
        // } else {
        //     executor.evm_mut().db_mut().bal_state.bal_builder = None;
        // }

        executor.apply_pre_execution_changes().map_err(BlockExecutionError::other)?;

        // if has_bal {
        //     executor.evm_mut().db_mut().bump_bal_index();
        // }

        for tx in block.transactions_recovered() {
            executor.execute_transaction(tx).map_err(BlockExecutionError::other)?;
            // if has_bal {
            //     executor.evm_mut().db_mut().bump_bal_index();
            // }
        }

        let result = executor.apply_post_execution_changes().map_err(BlockExecutionError::other)?;

        self.db.merge_transitions(BundleRetention::Reverts);

        Ok(alloy_block_execution_result_to_reth(result))
    }

    fn execute_one_with_state_hook<H>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: H,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        H: OnStateHook + 'static,
    {
        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;

        executor.evm_mut().db_mut().set_state_hook(Some(Box::new(state_hook)));

        let result = executor.execute_block(block.transactions_recovered());

        self.db.set_state_hook(None);
        self.db.merge_transitions(BundleRetention::Reverts);

        result.map(alloy_block_execution_result_to_reth).map_err(BlockExecutionError::other)
    }

    fn into_state(self) -> State<DB> {
        self.db
    }

    fn size_hint(&self) -> usize {
        self.db.bundle_state.size_hint()
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        // BAL execution is Amsterdam-only and remains stubbed for the evm2 pre-Amsterdam path.
        None
    }
}

/// Converts an alloy block execution result into Reth's local execution result type.
pub fn alloy_block_execution_result_to_reth<T>(
    result: alloy_evm::block::BlockExecutionResult<T>,
) -> BlockExecutionResult<T> {
    let alloy_evm::block::BlockExecutionResult { receipts, requests, gas_used, blob_gas_used } =
        result;
    BlockExecutionResult { receipts, requests, gas_used, blob_gas_used }
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

impl<L, R, Tx> RecoveredTx<Tx> for alloy_consensus::transaction::Either<L, R>
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

impl<'a, S, TxEnv, T> ExecutableTxParts<TxEnv, T> for &'a S
where
    S: ToTxEnv<TxEnv> + RecoveredTx<T>,
{
    type Recovered = &'a S;

    fn into_parts(self) -> (TxEnv, &'a S) {
        (self.to_tx_env(), self)
    }
}

impl<TxEnv, T: RecoveredTx<Tx>, Tx> ExecutableTxParts<TxEnv, Tx> for (TxEnv, T) {
    type Recovered = T;

    fn into_parts(self) -> (TxEnv, T) {
        (self.0, self.1)
    }
}

impl<T, TxEnv: FromRecoveredTx<T>> ExecutableTxParts<TxEnv, T> for Recovered<T> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: FromRecoveredTx<T>> ExecutableTxParts<TxEnv, T> for Recovered<&T> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: FromTxWithEncoded<T>> ExecutableTxParts<TxEnv, T> for WithEncoded<Recovered<T>> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: FromTxWithEncoded<T>> ExecutableTxParts<TxEnv, T> for WithEncoded<&Recovered<T>> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<L, R, TxEnv, T> ExecutableTxParts<TxEnv, T> for alloy_consensus::transaction::Either<L, R>
where
    L: ExecutableTxParts<TxEnv, T>,
    R: ExecutableTxParts<TxEnv, T>,
{
    type Recovered = alloy_consensus::transaction::Either<L::Recovered, R::Recovered>;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        match self {
            Self::Left(l) => {
                let (tx_env, recovered) = l.into_parts();
                (tx_env, alloy_consensus::transaction::Either::Left(recovered))
            }
            Self::Right(r) => {
                let (tx_env, recovered) = r.into_parts();
                (tx_env, alloy_consensus::transaction::Either::Right(recovered))
            }
        }
    }
}

/// Converts a revm bundle into Reth's evm2-backed bundle state type.
pub fn revm_bundle_to_evm2(bundle: BundleState, first_block: u64) -> Evm2BundleState {
    let BundleState { state, contracts, reverts, .. } = bundle;

    let accounts = state
        .iter()
        .map(|(address, account)| {
            (
                *address,
                Tracked {
                    original: account.original_info.as_ref().map(revm_account_info_to_evm2),
                    current: account.info.as_ref().map(revm_account_info_to_evm2),
                    _non_exhaustive: (),
                },
            )
        })
        .collect();

    let storage = state
        .iter()
        .filter_map(|(address, account)| {
            let slots = account
                .storage
                .iter()
                .map(|(slot, value)| {
                    (
                        *slot,
                        Tracked {
                            original: value.original_value(),
                            current: value.present_value(),
                            _non_exhaustive: (),
                        },
                    )
                })
                .collect();
            let storage = Evm2StorageChangeSet { wipe: account.was_destroyed(), slots };
            (!storage.slots.is_empty() || storage.wipe).then_some((*address, storage))
        })
        .collect::<AddressMap<_>>();

    let contracts = contracts
        .into_iter()
        .map(|(hash, bytecode)| (hash, Bytecode::new_raw(bytecode.original_bytes())))
        .collect();
    let block_reverts = reverts
        .iter()
        .map(|block_reverts| {
            let mut reverts = Evm2BlockReverts::default();
            for (address, revert) in block_reverts {
                match &revert.account {
                    AccountInfoRevert::DoNothing => {}
                    AccountInfoRevert::DeleteIt => {
                        reverts.accounts.insert(*address, None);
                    }
                    AccountInfoRevert::RevertTo(info) => {
                        reverts.accounts.insert(*address, Some(revm_account_info_to_evm2(info)));
                    }
                }

                let slots = revert
                    .storage
                    .iter()
                    .map(|(slot, value)| {
                        let value = match value {
                            RevertToSlot::Some(value) => *value,
                            RevertToSlot::Destroyed => Default::default(),
                        };
                        (*slot, value)
                    })
                    .collect();
                let storage =
                    Evm2StorageReverts { wiped: revert.wipe_storage, previous_wipe: false, slots };
                if storage.wiped || !storage.slots.is_empty() {
                    reverts.storage.insert(*address, storage);
                }
            }
            reverts
        })
        .collect();

    Evm2BundleState::from_parts(first_block, accounts, storage, contracts, block_reverts)
}

fn revm_account_info_to_evm2(info: &revm::state::AccountInfo) -> AccountInfo {
    AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: info.code.as_ref().map(|code| Bytecode::new_raw(code.original_bytes())),
        _non_exhaustive: (),
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
    use core::marker::PhantomData;
    use reth_ethereum_primitives::EthPrimitives;
    use revm::database::{CacheDB, EmptyDB};

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

        fn take_bal(&mut self) -> Option<Bytes> {
            None
        }
    }

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = CacheDB::<EmptyDB>::default();
        let executor = provider.executor(db);
        let _ = executor.execute(&Default::default());
    }
}
