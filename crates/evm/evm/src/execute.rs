//! Traits for execution.

use crate::{ConfigureEvm, Database, DynDatabase, EvmEnv, TxEnvFor};
use alloc::{boxed::Box, format, sync::Arc, vec::Vec};
use alloy_consensus::{
    transaction::{Either, Recovered, TransactionEnvelope},
    BlockHeader as _, Header,
};
use alloy_eip7928::{
    bal::DecodedBal, compute_block_access_list_hash_with_buf, BlockAccessIndex, BlockAccessList,
};
use alloy_eips::eip2718::{Typed2718, WithEncoded};
pub use alloy_evm::{block::CommitChanges, RecoveredTx};
use alloy_primitives::{Address, B256};
use core::fmt::Debug;
#[cfg(feature = "std")]
use evm2::evm::{CacheDB, Db};
use evm2::{evm::StateChangeSink as EvmStateChangeSink, registry::HandlerError, DatabaseError};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, EvmError, InternalBlockExecutionError,
    InvalidTxError,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_execution_types::{BlockExecutionResult, BundleSource, EvmState, HashedPostState};
#[cfg(feature = "std")]
use reth_primitives_traits::BlockTy;
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader, SignedTransaction,
    TxTy,
};
use reth_storage_api::StateProvider;
use reth_trie_common::updates::TrieUpdates;
use revm::database::BundleState;

/// Gas used by a successfully executed transaction.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GasOutput {
    /// Gas used for receipt accounting.
    tx_gas_used: u64,
    /// Regular gas used for Amsterdam block-capacity accounting.
    regular_gas_used: u64,
    /// Gas used for state-capacity accounting.
    state_gas_used: u64,
}

impl GasOutput {
    /// Creates a new gas output.
    pub const fn new(tx_gas_used: u64, state_gas_used: u64) -> Self {
        Self { tx_gas_used, regular_gas_used: tx_gas_used, state_gas_used }
    }

    /// Creates a gas output with independent receipt, regular, and state gas accounting.
    pub const fn new_with_regular(
        tx_gas_used: u64,
        regular_gas_used: u64,
        state_gas_used: u64,
    ) -> Self {
        Self { tx_gas_used, regular_gas_used, state_gas_used }
    }

    /// Returns transaction gas used for receipt accounting.
    pub const fn tx_gas_used(&self) -> u64 {
        self.tx_gas_used
    }

    /// Returns regular gas used for Amsterdam block-capacity accounting.
    pub const fn regular_gas_used(&self) -> u64 {
        self.regular_gas_used
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

/// Context for building a transaction receipt from an execution result.
#[derive(Debug, Clone)]
pub struct ReceiptBuilderCtx<TxType, TransactionResult> {
    /// Transaction type used by the receipt.
    pub tx_type: TxType,
    /// Raw transaction execution result.
    pub result: TransactionResult,
    /// Cumulative gas used by this transaction and all prior transactions in the block.
    pub cumulative_gas_used: u64,
}

/// Builds chain-specific receipts from raw transaction execution results.
#[auto_impl::auto_impl(&, Arc)]
pub trait ReceiptBuilder {
    /// Consensus transaction type accepted by the executor.
    type Transaction: SignedTransaction + TransactionEnvelope<TxType: Send + 'static>;
    /// Receipt produced by this builder.
    type Receipt: reth_primitives_traits::Receipt;

    /// Builds a receipt for the transaction execution result.
    fn build_receipt<T: evm2::EvmTypes>(
        &self,
        ctx: ReceiptBuilderCtx<
            <Self::Transaction as TransactionEnvelope>::TxType,
            evm2::TxResult<T>,
        >,
    ) -> Self::Receipt;
}

/// A detached block transaction output containing its raw execution result.
pub trait BlockTransactionResult<T: evm2::EvmTypes> {
    /// Returns the raw transaction execution result.
    fn result(&self) -> &evm2::TxResultWithState<T>;
}

/// A configured EVM instance.
pub trait Evm {
    /// Runtime EVM type family.
    type EvmTypes: evm2::EvmTypes<Tx = Self::Transaction>;
    /// Transaction environment consumed by this EVM.
    type Transaction;

    /// Executes a transaction without committing its state changes.
    fn transact(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResultWithState<Self::EvmTypes>, BlockExecutionError>;

    /// Executes and accepts writes in the EVM's block-scoped state without detaching them.
    fn transact_commit(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResult<Self::EvmTypes>, BlockExecutionError>;

    /// Executes a transaction and discards writes without materializing detached state.
    fn transact_result(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResult<Self::EvmTypes>, BlockExecutionError>;

    /// Executes a transaction with an inspector without committing its state changes.
    fn transact_with_inspector<I>(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
        inspector: I,
    ) -> Result<(I, evm2::TxResultWithState<Self::EvmTypes>), BlockExecutionError>
    where
        I: evm2::Inspector<Self::EvmTypes> + 'static;

    /// Sets the inspector used by subsequent transactions.
    fn set_inspector<I>(&mut self, inspector: I)
    where
        I: evm2::Inspector<Self::EvmTypes> + 'static;

    /// Returns the accepted state overlay, including earlier committed transactions.
    fn state_db_mut(&mut self) -> &mut dyn DynDatabase;

    /// Accepts detached changes after a tracing callback has consumed the pre-state.
    fn commit_state(&mut self, state: &evm2::evm::PendingState);

    /// Takes cached reads and accepted writes for a caller that continues using its database.
    fn take_state_cache(&mut self) -> evm2::evm::Cache;

    /// Returns active precompile addresses and identifiers.
    fn precompile_ids(&self) -> Vec<(Address, evm2::precompiles::PrecompileId)>;

    /// Returns whether an address is an active precompile.
    fn has_precompile(&self, address: &Address) -> bool;

    /// Returns account information visible through the accepted state overlay.
    fn account_info(
        &mut self,
        address: &Address,
    ) -> Result<Option<evm2::evm::AccountInfo>, BlockExecutionError>;

    /// Applies precompile address moves to the active precompile set.
    fn move_precompiles(
        &mut self,
        moves: impl IntoIterator<Item = (Address, Address)>,
    ) -> Result<(), evm2::precompiles::MovePrecompileError>;

    /// Executes a transaction and discards its writes while streaming observed state changes into
    /// `sink`.
    fn transact_and_discard<S>(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
        sink: &mut S,
    ) -> Result<(), BlockExecutionError>
    where
        S: EvmStateChangeSink,
        S::Error: Debug;
}

impl<'a, T: evm2::EvmTypes<Tx: Typed2718>> Evm for evm2::Evm<'a, T> {
    type EvmTypes = T;
    type Transaction = T::Tx;

    fn transact(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResultWithState<T>, BlockExecutionError> {
        evm2::Evm::transact(self, transaction)
            .map(|executed| executed.detach())
            .map_err(map_handler_error)
    }

    fn transact_commit(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResult<T>, BlockExecutionError> {
        execute_result(self, transaction, true)
    }

    fn transact_result(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
    ) -> Result<evm2::TxResult<T>, BlockExecutionError> {
        execute_result(self, transaction, false)
    }

    fn transact_with_inspector<I: evm2::Inspector<T> + 'a>(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
        inspector: I,
    ) -> Result<(I, evm2::TxResultWithState<T>), BlockExecutionError> {
        // TODO(dani): use either `&mut Inspector` or `clear_inspector_as`.
        evm2::Evm::set_inspector(self, inspector);
        let result = evm2::Evm::transact(self, transaction)
            .map(|executed| executed.detach())
            .map_err(map_handler_error);
        let inspector = self.clear_inspector().expect("inspector was set before execution");
        // SAFETY: the boxed inspector was created from `I` immediately above and was not replaced.
        let inspector = unsafe { Box::from_raw(Box::into_raw(inspector).cast::<I>()) };
        result.map(|result| (*inspector, result))
    }

    fn set_inspector<I: evm2::Inspector<T> + 'a>(&mut self, inspector: I) {
        evm2::Evm::set_inspector(self, inspector);
    }

    fn state_db_mut(&mut self) -> &mut dyn DynDatabase {
        self.overlay_db_mut()
    }

    fn commit_state(&mut self, state: &evm2::evm::PendingState) {
        self.overlay_db_mut().commit_pending(state);
    }

    fn take_state_cache(&mut self) -> evm2::evm::Cache {
        core::mem::take(&mut self.overlay_db_mut().cache)
    }

    fn precompile_ids(&self) -> Vec<(Address, evm2::precompiles::PrecompileId)> {
        self.precompiles().precompile_ids()
    }

    fn has_precompile(&self, address: &Address) -> bool {
        self.precompiles().contains(address)
    }

    fn account_info(
        &mut self,
        address: &Address,
    ) -> Result<Option<evm2::evm::AccountInfo>, BlockExecutionError> {
        self.state_mut().account_info_untracked(address).map_err(map_database_error)
    }

    fn move_precompiles(
        &mut self,
        moves: impl IntoIterator<Item = (Address, Address)>,
    ) -> Result<(), evm2::precompiles::MovePrecompileError> {
        self.precompiles_mut().move_precompiles(&moves.into_iter().collect::<Vec<_>>())
    }

    fn transact_and_discard<S>(
        &mut self,
        transaction: &Recovered<Self::Transaction>,
        sink: &mut S,
    ) -> Result<(), BlockExecutionError>
    where
        S: EvmStateChangeSink,
        S::Error: Debug,
    {
        let executed = self.transact(transaction).map_err(map_handler_error)?;

        executed.discard_with(sink).map(|_| ()).map_err(|err| {
            BlockExecutionError::msg(format!("discarded state sink failed: {err:?}"))
        })
    }
}

fn execute_result<T: evm2::EvmTypes<Tx: Typed2718>>(
    evm: &mut evm2::Evm<'_, T>,
    transaction: &Recovered<T::Tx>,
    commit: bool,
) -> Result<evm2::TxResult<T>, BlockExecutionError> {
    let executed = evm2::Evm::transact(evm, transaction).map_err(map_handler_error)?;
    Ok(if commit { executed.commit() } else { executed.discard() })
}

/// Converts a handler error into a block error without transaction hash context.
/// Validation errors retain the original handler error for downstream classification.
pub fn map_handler_error(error: HandlerError) -> BlockExecutionError {
    match error {
        HandlerError::Database(error) => map_database_error(error),
        HandlerError::Fatal(_) | HandlerError::WrongTransactionType { .. } => {
            BlockExecutionError::other(error)
        }
        error => BlockValidationError::Other(Box::new(error)).into(),
    }
}

/// Converts an owned database error using its internal-failure classification.
pub fn map_database_error(error: DatabaseError) -> BlockExecutionError {
    if error.is_fatal() {
        BlockExecutionError::other(error)
    } else {
        BlockValidationError::Other(Box::new(error)).into()
    }
}

/// A configured block executor.
pub trait BlockExecutor: Sized {
    /// Consensus transaction type executed by this executor.
    type Transaction;
    /// Receipt type produced by this executor.
    type Receipt;
    /// EVM instance used by this executor.
    type Evm: Evm;
    /// Owned transaction execution result and detached state changes.
    type TransactionResultWithState: BlockTransactionResult<<Self::Evm as Evm>::EvmTypes> + Send;
    /// EVM-native block access list used for indexed reads.
    type BlockAccessList: Send + Sync;

    /// Returns the underlying EVM.
    fn evm(&self) -> &Self::Evm;

    /// Returns the underlying EVM mutably.
    fn evm_mut(&mut self) -> &mut Self::Evm;

    /// Sets a hook for transaction state updates emitted during block execution.
    ///
    /// Returns `true` if the hook was installed.
    fn set_state_hook(&mut self, _hook: impl FnMut(EvmState) + Send + 'static) -> bool {
        false
    }

    /// Converts a canonical EIP-7928 block access list into the EVM-native representation.
    fn convert_block_access_list(
        block_access_list: &BlockAccessList,
    ) -> Result<Self::BlockAccessList, BlockExecutionError>;

    /// Attaches an EIP-7928 block access list for indexed state reads.
    fn set_block_access_list(&mut self, block_access_list: Arc<Self::BlockAccessList>);

    /// Sets the EIP-7928 block access index used by state reads and BAL construction.
    fn set_block_access_index(&mut self, index: BlockAccessIndex);

    /// Enables construction of an EIP-7928 block access list from committed state changes.
    fn enable_block_access_list_builder(&mut self);

    /// Takes the canonical EIP-7928 block access list built from committed state changes.
    fn take_block_access_list(&mut self) -> Option<BlockAccessList>;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the borrowed execution result, and commits
    /// changes when `f` returns [`CommitChanges::Yes`].
    fn execute_transaction_with_commit_condition(
        &mut self,
        transaction: impl ExecutorTx<Self>,
        f: impl FnOnce(&evm2::TxResult<<Self::Evm as Evm>::EvmTypes>) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let output = self.execute_transaction_without_commit(transaction)?;
        if f(&output.result().result).should_commit() {
            self.commit_transaction(output).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Validates transaction gas admission against the executor's current block or segment budget.
    /// Speculative workers and ordered commits must perform the same check before accepting a
    /// transaction's result or execution error.
    fn validate_transaction_gas_limit(&mut self, gas_limit: u64)
        -> Result<(), BlockExecutionError>;

    /// Executes a transaction and detaches its state changes without committing them.
    fn execute_transaction_without_commit(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<Self::TransactionResultWithState, BlockExecutionError>;

    /// Commits detached transaction state and records its receipt and gas accounting.
    fn commit_transaction(
        &mut self,
        output: Self::TransactionResultWithState,
    ) -> Result<GasOutput, BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the borrowed execution result, and commits
    /// changes.
    fn execute_transaction_with_result_closure(
        &mut self,
        transaction: impl ExecutorTx<Self>,
        f: impl FnOnce(&evm2::TxResult<<Self::Evm as Evm>::EvmTypes>),
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(transaction, |result| {
            f(result);
            CommitChanges::Yes
        })
        .map(Option::unwrap_or_default)
    }

    /// Executes a transaction and commits changes.
    fn execute_transaction(
        &mut self,
        transaction: impl ExecutorTx<Self>,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_result_closure(transaction, |_| {})
    }

    /// Returns receipts accumulated so far.
    fn receipts(&self) -> &[Self::Receipt];

    /// Finishes block execution and returns the output.
    fn finish_with_block_access_list(
        self,
    ) -> Result<(BlockExecutionOutput<Self::Receipt>, Option<BlockAccessList>), BlockExecutionError>;

    /// Finishes execution while retaining the prepared evm2 BAL for downstream consumers.
    ///
    /// Executors that build a native BAL can override this to avoid converting it back from the
    /// canonical representation.
    fn finish_with_prepared_block_access_list(
        self,
    ) -> Result<(BlockExecutionOutput<Self::Receipt>, Option<evm2::evm::Bal>), BlockExecutionError>
    {
        let (output, bal) = self.finish_with_block_access_list()?;
        let bal = bal
            .map(|bal| evm2::evm::Bal::try_from(bal.as_slice()))
            .transpose()
            .map_err(BlockExecutionError::other)?;
        Ok((output, bal))
    }

    /// Finishes block execution and returns the output.
    fn finish(self) -> Result<BlockExecutionOutput<Self::Receipt>, BlockExecutionError> {
        self.finish_with_block_access_list().map(|(output, _)| output)
    }
}

/// A type that creates configured block executors.
pub trait BlockExecutorFactory {
    /// Additional EVM factory configuration owned by this executor factory.
    type EvmFactory;
    /// Runtime EVM type family.
    type EvmTypes: evm2::EvmTypes<TxResultExt: Send>;
    /// Consensus transaction type consumed by executors from this factory.
    type Transaction: Debug + Clone + Send + Sync + 'static;
    /// Receipt type produced by executors from this factory.
    type Receipt;
    /// EVM instance consumed by executors from this factory.
    type Evm<'a>: Evm<
        EvmTypes = Self::EvmTypes,
        Transaction = <Self::EvmTypes as evm2::EvmTypesHost>::Tx,
    >;
    /// EVM environment consumed by this factory.
    type EvmEnv: EvmEnv<EvmTypes = Self::EvmTypes>;
    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;
    /// Block executor returned by this factory.
    type Executor<'a>: BlockExecutor<
        Transaction = Self::Transaction,
        Receipt = Self::Receipt,
        Evm = Self::Evm<'a>,
        BlockAccessList = evm2::evm::Bal,
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

    /// Returns the configured EVM factory state.
    fn evm_factory(&self) -> &Self::EvmFactory;

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
    /// Transactions that were executed in this block.
    pub transactions: Vec<F::Transaction>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<F::Receipt>,
    /// Execution state after block execution.
    pub execution_state: &'b BundleState,
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
        transactions: Vec<F::Transaction>,
        output: &'b BlockExecutionResult<F::Receipt>,
        execution_state: &'b BundleState,
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
    /// Changed state produced by block execution.
    pub execution_state: BundleState,
    /// Hashed state after execution.
    pub hashed_state: HashedPostState,
    /// Trie updates collected during state-root calculation.
    pub trie_updates: TrieUpdates,
    /// The built block.
    pub block: RecoveredBlock<N::Block>,
    /// Block access list built during execution (EIP-7928, Amsterdam).
    pub block_access_list: Option<DecodedBal>,
}

/// A type that knows how to execute transactions and assemble a block.
pub trait BlockBuilder: Sized {
    /// The primitive types used by the inner [`BlockExecutor`].
    type Primitives: NodePrimitives;
    /// Inner block executor.
    type Executor: BlockExecutor<
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
        Evm: Evm<Transaction: FromTxWithEncoded<TxTy<Self::Primitives>>>,
    >;
    /// EVM environment used for block execution.
    type EvmEnv: EvmEnv;

    /// Returns the EVM environment used for block execution.
    fn evm_env(&self) -> &Self::EvmEnv;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a transaction, exposes its borrowed execution result to `f`, and saves it
    /// for block assembly only if committed.
    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(
            &evm2::TxResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::EvmTypes>,
        ) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError>;

    /// Executes a transaction, invokes `f` with the detached result and state changes, and saves
    /// it for block assembly.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::TransactionResultWithState),
    ) -> Result<GasOutput, BlockExecutionError>;

    /// Executes a transaction and saves it for block assembly.
    fn execute_transaction(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(tx, |_| CommitChanges::Yes)
            .map(Option::unwrap_or_default)
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

    /// Sets a hook for transaction state updates emitted while building a block.
    ///
    /// Returns `true` if the hook was installed.
    fn set_state_hook(&mut self, _hook: impl FnMut(EvmState) + Send + 'static) -> bool {
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
    pub transactions: Vec<Recovered<TxTy<N>>>,
    /// Block execution context.
    pub ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<HeaderTy<N>>,
    /// Block assembler.
    pub assembler: &'a Assembler,
}

impl<'a, F, Assembler, N> BasicBlockBuilder<'a, F, F::Executor<'a>, Assembler, N>
where
    F: BlockExecutorFactory<Transaction = TxTy<N>, Receipt = ReceiptTy<N>> + 'a,
    Assembler: BlockAssembler<F, Block = N::Block> + 'a,
    N: NodePrimitives,
{
    /// Creates a block builder that accumulates bundle state in the execution output.
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
            ctx,
            parent,
            assembler,
        }
    }
}

/// Executable transaction consumed by a block executor.
pub trait ExecutorTx<Executor: BlockExecutor>:
    ExecutableTxParts<Recovered<<Executor::Evm as Evm>::Transaction>, Executor::Transaction>
{
}

impl<T, Executor> ExecutorTx<Executor> for T
where
    Executor: BlockExecutor,
    T: ExecutableTxParts<Recovered<<Executor::Evm as Evm>::Transaction>, Executor::Transaction>,
{
}

impl<'a, F, Executor, Assembler, N> BlockBuilder
    for BasicBlockBuilder<'a, F, Executor, Assembler, N>
where
    F: BlockExecutorFactory<Transaction = TxTy<N>, Receipt = ReceiptTy<N>>,
    Executor: BlockExecutor<
        Transaction = TxTy<N>,
        Receipt = ReceiptTy<N>,
        Evm: Evm<EvmTypes = F::EvmTypes>,
    >,
    Assembler: BlockAssembler<F, Block = N::Block>,
    N: NodePrimitives,
    TxTy<N>: Clone,
    <<Executor as BlockExecutor>::Evm as Evm>::Transaction: FromTxWithEncoded<TxTy<N>>,
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
        f: impl FnOnce(
            &evm2::TxResult<<<Self::Executor as BlockExecutor>::Evm as Evm>::EvmTypes>,
        ) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        let tx = Recovered::new_unchecked(tx.tx().clone(), *tx.signer());
        if let Some(output) =
            self.executor.execute_transaction_with_commit_condition((tx_env, &tx), f)?
        {
            self.transactions.push(tx);
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }

    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::TransactionResultWithState),
    ) -> Result<GasOutput, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        let tx = Recovered::new_unchecked(tx.tx().clone(), *tx.signer());
        let output = self.executor.execute_transaction_without_commit((tx_env, &tx))?;
        f(&output);
        let gas_output = self.executor.commit_transaction(output)?;
        self.transactions.push(tx);
        Ok(gas_output)
    }

    fn finish_with_state_root(
        self,
        state_provider: impl StateProvider,
        state_root: impl FnOnce(
            &BlockExecutionOutput<ReceiptTy<Self::Primitives>>,
        ) -> Result<Option<(B256, TrieUpdates)>, BlockExecutionError>,
    ) -> Result<BlockBuilderOutcome<N>, BlockExecutionError> {
        let Self { executor, evm_env, transactions, ctx, parent, assembler } = self;

        let (output, block_access_list) = executor.finish_with_block_access_list()?;
        let block_access_list = block_access_list.map(|bal| {
            let mut raw = Vec::new();
            let hash = compute_block_access_list_hash_with_buf(&bal, &mut raw);
            DecodedBal::new_unchecked(bal.into(), raw.into(), hash)
        });
        let block_access_list_hash = block_access_list.as_ref().map(DecodedBal::hash);
        let hashed_state =
            state_provider.hashed_post_state(&output.state).map_err(BlockExecutionError::other)?;
        let (state_root, trie_updates) = match state_root(&output)? {
            Some(precomputed) => precomputed,
            None => state_provider
                .state_root_with_updates(hashed_state.clone())
                .map_err(BlockExecutionError::other)?,
        };
        let (transactions, senders) = transactions.into_iter().map(|tx| tx.into_parts()).unzip();

        let block = assembler.assemble_block(BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            parent,
            transactions,
            output: &output.result,
            execution_state: &output.state,
            state_provider: &state_provider,
            state_root,
            block_access_list_hash,
        })?;
        let block = RecoveredBlock::new_unhashed(block, senders);

        Ok(BlockBuilderOutcome {
            execution_result: output.result,
            execution_state: output.state,
            hashed_state,
            trie_updates,
            block,
            block_access_list,
        })
    }

    fn set_state_hook(&mut self, hook: impl FnMut(EvmState) + Send + 'static) -> bool {
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

/// A type that knows how to execute blocks over a database.
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

    /// Executes a single block and emits transaction state updates to the provided hook.
    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(EvmState) + Send + 'static;

    /// Consumes the type and executes the block.
    fn execute(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let result = self.execute_one(block)?;
        let state = self.into_state();
        Ok(BlockExecutionOutput::new(result, state))
    }

    /// Consumes the type, executes the block, and emits transaction state updates to the provided
    /// hook.
    fn execute_with_state_hook<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(EvmState) + Send + 'static,
    {
        let result = self.execute_one_with_state_hook(block, state_hook)?;
        let state = self.into_state();
        Ok(BlockExecutionOutput::new(result, state))
    }

    /// Executes the block and invokes `f` with the accumulated execution state after execution.
    fn execute_with_state_closure<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        mut f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&BundleState),
    {
        let result = self.execute_one(block)?;
        let state = self.into_state();
        f(&state);

        Ok(BlockExecutionOutput::new(result, state))
    }

    /// Executes the block and always invokes `f` with the accumulated execution state, even when
    /// execution fails.
    fn execute_with_state_closure_always<F>(
        mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        mut f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&BundleState),
    {
        let result = self.execute_one(block);
        let state = self.into_state();
        f(&state);

        Ok(BlockExecutionOutput::new(result?, state))
    }

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
    fn into_state(self) -> BundleState;

    /// Takes the canonical block access list from executor.
    fn take_bal(&mut self) -> Option<BlockAccessList>;
}

/// Generic block executor backed by a [`ConfigureEvm`] implementation.
#[cfg(feature = "std")]
#[expect(missing_debug_implementations)]
pub struct BasicBlockExecutor<Evm, DB: Database> {
    evm_config: Evm,
    batch_database: CacheDB<Db<DB>>,
    batch_state: BundleState,
    block_access_list: Option<BlockAccessList>,
}

#[cfg(feature = "std")]
impl<Evm, DB: Database> BasicBlockExecutor<Evm, DB> {
    /// Creates a new generic block executor.
    pub fn new(evm_config: Evm, database: DB) -> Self {
        Self {
            evm_config,
            batch_database: CacheDB::new(Db::new(database)),
            batch_state: BundleState::default(),
            block_access_list: None,
        }
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> BasicBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database,
{
    fn merge_batch_output(
        mut batch_state: BundleState,
        output: BlockExecutionOutput<ReceiptTy<Evm::Primitives>>,
    ) -> BlockExecutionOutput<ReceiptTy<Evm::Primitives>> {
        if batch_state.reverts.is_empty() {
            return output
        }

        let result = output.result;
        let state = output.state;
        batch_state.extend(state);
        BlockExecutionOutput::new(result, batch_state)
    }

    #[expect(clippy::type_complexity)]
    fn execute_block_with_database(
        evm_config: &Evm,
        block: &RecoveredBlock<BlockTy<Evm::Primitives>>,
        database: impl DynDatabase,
    ) -> Result<
        (BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, Option<BlockAccessList>),
        BlockExecutionError,
    > {
        Self::execute_block_with_database_and_state_hook(evm_config, block, database, None)
    }

    #[expect(clippy::type_complexity)]
    fn execute_block_with_database_and_state_hook(
        evm_config: &Evm,
        block: &RecoveredBlock<BlockTy<Evm::Primitives>>,
        database: impl DynDatabase,
        state_hook: Option<Box<dyn FnMut(EvmState) + Send>>,
    ) -> Result<
        (BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, Option<BlockAccessList>),
        BlockExecutionError,
    > {
        let evm_env = evm_config.evm_env(block.header()).map_err(BlockExecutionError::other)?;
        let evm = evm_config.block_executor_factory().evm_with_env(database, evm_env);
        let ctx = evm_config
            .context_for_block(block.sealed_block())
            .map_err(BlockExecutionError::other)?;
        let mut executor = evm_config.block_executor_factory().create_executor(evm, ctx);
        if block.header().block_access_list_hash().is_some() {
            executor.enable_block_access_list_builder();
        }
        if let Some(hook) = state_hook &&
            !executor.set_state_hook(hook)
        {
            return Err(BlockExecutionError::msg("block executor does not support state hooks"))
        }

        executor.apply_pre_execution_changes()?;
        for transaction in block.transactions_recovered() {
            executor.execute_transaction(transaction)?;
        }
        let (output, block_access_list) = executor.finish_with_block_access_list()?;
        Ok((output, block_access_list))
    }
}

#[cfg(feature = "std")]
impl<Evm, DB> Executor<DB> for BasicBlockExecutor<Evm, DB>
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
        let (output, block_access_list) =
            Self::execute_block_with_database(&self.evm_config, block, &mut self.batch_database)?;
        self.block_access_list = block_access_list;
        self.batch_database.commit_source(&BundleSource(&output.state));

        let block_state = output.state;
        self.batch_state.extend(block_state);

        Ok(output.result)
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(EvmState) + Send + 'static,
    {
        let (output, block_access_list) = Self::execute_block_with_database_and_state_hook(
            &self.evm_config,
            block,
            &mut self.batch_database,
            Some(Box::new(state_hook)),
        )?;
        self.block_access_list = block_access_list;
        self.batch_database.commit_source(&BundleSource(&output.state));

        let block_state = output.state;
        self.batch_state.extend(block_state);

        Ok(output.result)
    }

    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let Self { evm_config, batch_database, batch_state, .. } = self;
        let (output, _) = Self::execute_block_with_database(&evm_config, block, batch_database)?;
        Ok(Self::merge_batch_output(batch_state, output))
    }

    fn execute_with_state_hook<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(EvmState) + Send + 'static,
    {
        let Self { evm_config, batch_database, batch_state, .. } = self;
        let (output, _) = Self::execute_block_with_database_and_state_hook(
            &evm_config,
            block,
            batch_database,
            Some(Box::new(state_hook)),
        )?;
        Ok(Self::merge_batch_output(batch_state, output))
    }

    fn size_hint(&self) -> usize {
        self.batch_state.size_hint()
    }

    fn into_state(self) -> BundleState {
        self.batch_state
    }

    fn take_bal(&mut self) -> Option<BlockAccessList> {
        self.block_access_list.take()
    }
}

/// Executor returned by configurations that do not support block execution in the active build.
#[cfg(not(feature = "std"))]
pub(crate) struct UnsupportedExecutor<N> {
    _marker: core::marker::PhantomData<N>,
}

#[cfg(not(feature = "std"))]
impl<N> Default for UnsupportedExecutor<N> {
    fn default() -> Self {
        Self { _marker: core::marker::PhantomData }
    }
}

#[cfg(not(feature = "std"))]
impl<N, DB> Executor<DB> for UnsupportedExecutor<N>
where
    N: NodePrimitives,
    DB: Database,
{
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
        F: FnMut(EvmState) + Send + 'static,
    {
        Err(BlockExecutionError::msg("block execution is unsupported by this EVM configuration"))
    }

    fn execute_with_state_hook<F>(
        self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(EvmState) + Send + 'static,
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

    fn into_state(self) -> BundleState {
        BundleState::default()
    }

    fn take_bal(&mut self) -> Option<BlockAccessList> {
        None
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

/// Converts a recovered transaction into the configured transaction environment.
pub trait FromRecoveredTx<T> {
    /// Converts a recovered transaction into the configured transaction environment.
    fn from_recovered_tx(tx: Recovered<T>) -> Self;
}

impl<T> FromRecoveredTx<T> for evm2::ethereum::TxEnvelope
where
    Self: From<T>,
{
    fn from_recovered_tx(tx: Recovered<T>) -> Self {
        tx.into_inner().into()
    }
}

impl<T, TxEnv> FromRecoveredTx<T> for Recovered<TxEnv>
where
    TxEnv: FromRecoveredTx<T>,
{
    fn from_recovered_tx(tx: Recovered<T>) -> Self {
        let signer = tx.signer();
        Self::new_unchecked(TxEnv::from_recovered_tx(tx), signer)
    }
}

/// Converts a recovered transaction with its encoded bytes into the configured transaction
/// environment.
pub trait FromTxWithEncoded<T>: FromRecoveredTx<T> {
    /// Converts a recovered transaction with its encoded bytes into the configured transaction
    /// environment.
    fn from_tx_with_encoded(tx: WithEncoded<Recovered<T>>) -> Self
    where
        Self: Sized,
    {
        Self::from_recovered_tx(tx.1)
    }
}

impl<T> FromTxWithEncoded<T> for evm2::ethereum::TxEnvelope where Self: From<T> {}

impl<T, TxEnv> FromTxWithEncoded<T> for Recovered<TxEnv>
where
    TxEnv: FromTxWithEncoded<T>,
{
    fn from_tx_with_encoded(tx: WithEncoded<Recovered<T>>) -> Self {
        let signer = tx.1.signer();
        Self::new_unchecked(TxEnv::from_tx_with_encoded(tx), signer)
    }
}

/// Converts transaction wrappers into the configured transaction environment.
pub trait IntoTxEnv<TxEnv> {
    /// Converts this transaction wrapper into the configured transaction environment.
    fn into_tx_env(self) -> TxEnv;
}

impl<T, TxEnv> IntoTxEnv<TxEnv> for Recovered<T>
where
    TxEnv: FromRecoveredTx<T>,
{
    fn into_tx_env(self) -> TxEnv {
        TxEnv::from_recovered_tx(self)
    }
}

impl<T, TxEnv> IntoTxEnv<TxEnv> for WithEncoded<Recovered<T>>
where
    TxEnv: FromTxWithEncoded<T>,
{
    fn into_tx_env(self) -> TxEnv {
        TxEnv::from_tx_with_encoded(self)
    }
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for Recovered<T>
where
    TxEnv: FromRecoveredTx<T>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (TxEnv::from_recovered_tx(self.clone()), self)
    }
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for Recovered<&T>
where
    TxEnv: FromRecoveredTx<T>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (
            TxEnv::from_recovered_tx(Recovered::new_unchecked(
                (*self.inner()).clone(),
                *self.signer_ref(),
            )),
            self,
        )
    }
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for WithEncoded<Recovered<T>>
where
    TxEnv: FromTxWithEncoded<T>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (TxEnv::from_tx_with_encoded(self.clone()), self)
    }
}

impl<TxEnv, Tx, T> ExecutableTxParts<TxEnv, Tx> for (TxEnv, T)
where
    T: RecoveredTx<Tx>,
{
    type Recovered = T;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        self
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
