//! Traits for execution.

use crate::{ConfigureEvm, Database, Evm, EvmFactory, OnStateHook, StateHookExt, TxEnvFor};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_consensus::{BlockHeader, Header};
use alloy_eip7928::{compute_block_access_list_hash, BlockAccessList};
use alloy_eips::eip2718::WithEncoded;
use alloy_evm::{
    block::{
        BalIndexedDatabase, BlockExecutionError as AlloyBlockExecutionError,
        BlockExecutionResult as AlloyBlockExecutionResult, BlockExecutor as AlloyBlockExecutor,
        BlockExecutorFactory as AlloyBlockExecutorFactory,
        BlockValidationError as AlloyBlockValidationError, CommitChanges as AlloyCommitChanges,
        GasOutput as AlloyGasOutput,
        InternalBlockExecutionError as AlloyInternalBlockExecutionError, StateDB,
        TxResult as AlloyTxResult,
    },
    EvmEnv, RecoveredTx, ToTxEnv,
};
use alloy_primitives::{map::AddressHashSet, Address, B256};
use evm2::{evm::precompile::PrecompileProvider, BaseEvmTypes};
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::{
    AccountInfoRevert, AccountStatus, Bal, BlockExecutionResult, BundleRetention, BundleState,
    State,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, Recovered, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;
pub use reth_storage_errors::provider::ProviderError;
use reth_trie_common::{updates::TrieUpdates, HashedPostState};
use revm::{
    context::{result::ResultAndState, CfgEnv},
    context_interface::either::Either,
    inspector::NoOpInspector,
    Inspector,
};

/// Converts a temporary alloy block execution error into reth's owned execution error type.
pub fn convert_alloy_block_execution_error(error: AlloyBlockExecutionError) -> BlockExecutionError {
    match error {
        AlloyBlockExecutionError::Validation(error) => {
            BlockExecutionError::Validation(convert_alloy_block_validation_error(error))
        }
        AlloyBlockExecutionError::Internal(error) => {
            BlockExecutionError::Internal(convert_alloy_internal_block_execution_error(error))
        }
    }
}

/// Converts a temporary alloy block execution result into reth's owned execution result type.
pub fn convert_alloy_block_execution_result<T>(
    result: AlloyBlockExecutionResult<T>,
) -> BlockExecutionResult<T> {
    let AlloyBlockExecutionResult { receipts, requests, gas_used, blob_gas_used } = result;
    BlockExecutionResult { receipts, requests, gas_used, blob_gas_used }
}

/// Gas used by a transaction, split into regular and state gas components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GasOutput {
    /// Gas used by the transaction. This value is found in the receipt.
    tx_gas_used: u64,
    /// State gas used by the transaction.
    state_gas_used: u64,
}

impl GasOutput {
    /// Creates a new [`GasOutput`] with the given regular gas used.
    pub const fn new(tx_gas_used: u64) -> Self {
        Self { tx_gas_used, state_gas_used: 0 }
    }

    /// Creates a new [`GasOutput`] with both regular and state gas.
    pub const fn with_state_gas(tx_gas_used: u64, state_gas_used: u64) -> Self {
        Self { tx_gas_used, state_gas_used }
    }

    /// Returns the regular gas used.
    pub const fn tx_gas_used(&self) -> u64 {
        self.tx_gas_used
    }

    /// Returns the state gas used.
    pub const fn state_gas_used(&self) -> u64 {
        self.state_gas_used
    }
}

impl From<AlloyGasOutput> for GasOutput {
    fn from(output: AlloyGasOutput) -> Self {
        Self::with_state_gas(output.tx_gas_used(), output.state_gas_used())
    }
}

impl From<GasOutput> for AlloyGasOutput {
    fn from(output: GasOutput) -> Self {
        Self::with_state_gas(output.tx_gas_used(), output.state_gas_used())
    }
}

/// Helper trait to encapsulate requirements for block executor transaction input.
pub trait ExecutableTxParts<TxEnv, T> {
    /// The recovered transaction accessor type.
    type Recovered: RecoveredTx<T>;

    /// Converts the transaction into an executable transaction environment and recovered
    /// transaction.
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

impl<T, TxEnv: alloy_evm::FromRecoveredTx<T>> ExecutableTxParts<TxEnv, T> for Recovered<T> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: alloy_evm::FromRecoveredTx<T>> ExecutableTxParts<TxEnv, T> for Recovered<&T> {
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: alloy_evm::FromTxWithEncoded<T>> ExecutableTxParts<TxEnv, T>
    for WithEncoded<Recovered<T>>
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
    }
}

impl<T, TxEnv: alloy_evm::FromTxWithEncoded<T>> ExecutableTxParts<TxEnv, T>
    for WithEncoded<&Recovered<T>>
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.to_tx_env(), self)
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
                let (env, rec) = l.into_parts();
                (env, Either::Left(rec))
            }
            Self::Right(r) => {
                let (env, rec) = r.into_parts();
                (env, Either::Right(rec))
            }
        }
    }
}

/// Alias for [`ExecutableTxParts`] with types associated with the given [`BlockExecutor`].
pub trait ExecutableTx<E: BlockExecutor + ?Sized>:
    ExecutableTxParts<<E::Evm as Evm>::Tx, E::Transaction>
{
}

impl<E: BlockExecutor + ?Sized, T> ExecutableTx<E> for T where
    T: ExecutableTxParts<<E::Evm as Evm>::Tx, E::Transaction>
{
}

/// Marks whether transaction changes should be committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CommitChanges {
    /// Transaction should be committed into block executor state.
    Yes,
    /// Transaction should not be committed.
    No,
}

impl CommitChanges {
    /// Returns `true` if changes should be committed.
    pub const fn should_commit(self) -> bool {
        matches!(self, Self::Yes)
    }
}

impl From<CommitChanges> for AlloyCommitChanges {
    fn from(commit: CommitChanges) -> Self {
        match commit {
            CommitChanges::Yes => Self::Yes,
            CommitChanges::No => Self::No,
        }
    }
}

impl From<AlloyCommitChanges> for CommitChanges {
    fn from(commit: AlloyCommitChanges) -> Self {
        match commit {
            AlloyCommitChanges::Yes => Self::Yes,
            AlloyCommitChanges::No => Self::No,
        }
    }
}

/// A type that knows how to execute a single block.
pub trait BlockExecutor {
    /// Input transaction type.
    type Transaction;
    /// Receipt type this executor produces.
    type Receipt;
    /// EVM used by the executor.
    type Evm: Evm<
        Tx: alloy_evm::FromRecoveredTx<Self::Transaction>
                + alloy_evm::FromTxWithEncoded<Self::Transaction>,
        >;
    /// State database used by the executor.
    type DB;
    /// Result of a transaction execution.
    type Result: TxResult<HaltReason = <Self::Evm as Evm>::HaltReason>;

    /// Applies any necessary changes before executing block transactions.
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError>;

    /// Executes a single transaction and commits the result.
    fn execute_transaction(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_result_closure(tx, |_| ())
    }

    /// Executes a transaction at an explicit index and commits the result.
    fn execute_transaction_with_index(
        &mut self,
        tx: impl ExecutableTx<Self>,
        tx_index: usize,
    ) -> Result<GasOutput, BlockExecutionError>
    where
        Self::DB: BalIndexedDatabase,
    {
        self.execute_transaction_with_index_and_result_closure(tx, tx_index, |_| ())
    }

    /// Executes a transaction at an explicit index and exposes the result before commit.
    fn execute_transaction_with_index_and_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>,
        tx_index: usize,
        f: impl FnOnce(&Self::Result),
    ) -> Result<GasOutput, BlockExecutionError>
    where
        Self::DB: BalIndexedDatabase,
    {
        self.db_mut().set_bal_index(tx_index as u64 + 1);
        self.execute_transaction_with_result_closure(tx, f)
    }

    /// Executes a single transaction and exposes the result before commit.
    fn execute_transaction_with_result_closure(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&Self::Result),
    ) -> Result<GasOutput, BlockExecutionError> {
        self.execute_transaction_with_commit_condition(tx, |res| {
            f(res);
            CommitChanges::Yes
        })
        .map(Option::unwrap_or_default)
    }

    /// Executes a single transaction and commits conditionally.
    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&Self::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let output = self.execute_transaction_without_commit(tx)?;
        if !f(&output).should_commit() {
            return Ok(None);
        }
        Ok(Some(self.commit_transaction(output)))
    }

    /// Executes a transaction without committing state changes.
    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError>;

    /// Commits a previously executed transaction.
    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput;

    /// Applies post execution changes and returns the EVM with block execution result.
    fn finish(
        self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError>;

    /// Applies post execution changes and returns only the execution result.
    fn apply_post_execution_changes(
        self,
    ) -> Result<BlockExecutionResult<Self::Receipt>, BlockExecutionError>
    where
        Self: Sized,
    {
        self.finish().map(|(_, result)| result)
    }

    /// Exposes mutable reference to EVM.
    fn evm_mut(&mut self) -> &mut Self::Evm;

    /// Exposes immutable reference to EVM.
    fn evm(&self) -> &Self::Evm;

    /// Exposes mutable reference to the executor state database.
    fn db_mut(&mut self) -> &mut Self::DB;

    /// Exposes immutable reference to the executor state database.
    fn db(&self) -> &Self::DB;

    /// Exposes the block environment used by this executor.
    fn block_env(&self) -> &<Self::Evm as Evm>::BlockEnv {
        self.evm().block()
    }

    /// Exposes the configuration environment used by this executor.
    fn cfg_env(&self) -> &CfgEnv<<Self::Evm as Evm>::Spec> {
        self.evm().cfg_env()
    }

    /// Returns recorded receipts.
    fn receipts(&self) -> &[Self::Receipt];

    /// Replaces the executor's evm2 precompile provider, if supported.
    fn set_precompiles<P>(&mut self, _precompiles: P)
    where
        P: PrecompileProvider<BaseEvmTypes>,
    {
    }

    /// Executes all transactions in a block.
    fn execute_block(
        mut self,
        transactions: impl IntoIterator<Item = impl ExecutableTx<Self>>,
    ) -> Result<BlockExecutionResult<Self::Receipt>, BlockExecutionError>
    where
        Self: Sized,
    {
        self.apply_pre_execution_changes()?;
        for tx in transactions {
            self.execute_transaction(tx)?;
        }
        self.apply_post_execution_changes()
    }
}

impl<T> BlockExecutor for T
where
    T: AlloyBlockExecutor,
{
    type Transaction = T::Transaction;
    type Receipt = T::Receipt;
    type Evm = T::Evm;
    type DB = <T::Evm as Evm>::DB;
    type Result = T::Result;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        AlloyBlockExecutor::apply_pre_execution_changes(self)
            .map_err(convert_alloy_block_execution_error)
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (tx_env, recovered) = tx.into_parts();
        AlloyBlockExecutor::execute_transaction_without_commit(self, (tx_env, recovered))
            .map_err(convert_alloy_block_execution_error)
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&Self::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (tx_env, recovered) = tx.into_parts();
        AlloyBlockExecutor::execute_transaction_with_commit_condition(
            self,
            (tx_env, recovered),
            |result| f(result).into(),
        )
        .map(|gas| gas.map(Into::into))
        .map_err(convert_alloy_block_execution_error)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput {
        AlloyBlockExecutor::commit_transaction(self, output).into()
    }

    fn finish(
        self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        AlloyBlockExecutor::finish(self)
            .map(|(evm, result)| (evm, convert_alloy_block_execution_result(result)))
            .map_err(convert_alloy_block_execution_error)
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        AlloyBlockExecutor::evm_mut(self)
    }

    fn evm(&self) -> &Self::Evm {
        AlloyBlockExecutor::evm(self)
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        self.evm_mut().db_mut()
    }

    fn db(&self) -> &Self::DB {
        self.evm().db()
    }

    fn receipts(&self) -> &[Self::Receipt] {
        AlloyBlockExecutor::receipts(self)
    }
}

/// A result of transaction execution.
pub trait TxResult: Send + 'static {
    /// Halt reason.
    type HaltReason: Send + 'static;

    /// Returns the inner EVM result.
    fn result(&self) -> &ResultAndState<Self::HaltReason>;

    /// Consumes self and returns the inner EVM result.
    fn into_result(self) -> ResultAndState<Self::HaltReason>;
}

impl<T> TxResult for T
where
    T: AlloyTxResult,
{
    type HaltReason = T::HaltReason;

    fn result(&self) -> &ResultAndState<Self::HaltReason> {
        AlloyTxResult::result(self)
    }

    fn into_result(self) -> ResultAndState<Self::HaltReason> {
        AlloyTxResult::into_result(self)
    }
}

/// Helper alias for executors produced by a [`BlockExecutorFactory`].
pub type BlockExecutorFor<'a, F, DB, I = NoOpInspector> =
    <F as BlockExecutorFactory>::Executor<'a, DB, I>;

/// A factory that can create [`BlockExecutor`]s.
pub trait BlockExecutorFactory: 'static {
    /// The EVM factory used by the executor.
    type EvmFactory: EvmFactory;

    /// Result type produced by the executor for each transaction.
    type TxExecutionResult: TxResult<HaltReason = <Self::EvmFactory as EvmFactory>::HaltReason>;

    /// Context required for block execution beyond what the EVM provides.
    type ExecutionCtx<'a>: Clone;

    /// Transaction type used by the executor.
    type Transaction;

    /// Receipt type produced by the executor.
    type Receipt;

    /// The executor type this factory produces.
    type Executor<'a, DB: StateDB, I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>>: BlockExecutor<
        Evm = <Self::EvmFactory as EvmFactory>::Evm<DB, I>,
        DB = DB,
        Transaction = Self::Transaction,
        Receipt = Self::Receipt,
        Result = Self::TxExecutionResult,
    >;

    /// Reference to EVM factory used by the executor.
    fn evm_factory(&self) -> &Self::EvmFactory;

    /// Creates an executor with given EVM and execution context.
    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <Self::EvmFactory as EvmFactory>::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>;

    /// Creates an executor with the given database, environment, and execution context.
    fn create_executor_with_env<'a, DB>(
        &'a self,
        db: DB,
        evm_env: EvmEnv<
            <Self::EvmFactory as EvmFactory>::Spec,
            <Self::EvmFactory as EvmFactory>::BlockEnv,
        >,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, NoOpInspector>
    where
        DB: StateDB,
    {
        let evm = self.evm_factory().create_evm(db, evm_env);
        self.create_executor(evm, ctx)
    }
}

impl<T> BlockExecutorFactory for T
where
    T: AlloyBlockExecutorFactory,
{
    type EvmFactory = T::EvmFactory;
    type TxExecutionResult = T::TxExecutionResult;
    type ExecutionCtx<'a> = T::ExecutionCtx<'a>;
    type Transaction = T::Transaction;
    type Receipt = T::Receipt;
    type Executor<'a, DB: StateDB, I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>> =
        T::Executor<'a, DB, I>;

    fn evm_factory(&self) -> &Self::EvmFactory {
        AlloyBlockExecutorFactory::evm_factory(self)
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <Self::EvmFactory as EvmFactory>::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>,
    {
        AlloyBlockExecutorFactory::create_executor(self, evm, ctx)
    }
}

fn convert_alloy_block_validation_error(error: AlloyBlockValidationError) -> BlockValidationError {
    match error {
        AlloyBlockValidationError::InvalidTx { hash, error } => {
            BlockValidationError::InvalidTx { hash, error }
        }
        AlloyBlockValidationError::EVM { hash, error } => BlockValidationError::EVM { hash, error },
        AlloyBlockValidationError::IncrementBalanceFailed => {
            BlockValidationError::IncrementBalanceFailed
        }
        AlloyBlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
            transaction_gas_limit,
            block_available_gas,
        } => BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
            transaction_gas_limit,
            block_available_gas,
        },
        AlloyBlockValidationError::MissingParentBeaconBlockRoot => {
            BlockValidationError::MissingParentBeaconBlockRoot
        }
        AlloyBlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
            parent_beacon_block_root,
        } => BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
            parent_beacon_block_root,
        },
        AlloyBlockValidationError::BeaconRootContractCall { parent_beacon_block_root, message } => {
            BlockValidationError::BeaconRootContractCall { parent_beacon_block_root, message }
        }
        AlloyBlockValidationError::BlockHashContractCall { message } => {
            BlockValidationError::BlockHashContractCall { message }
        }
        AlloyBlockValidationError::WithdrawalRequestsContractCall { message } => {
            BlockValidationError::WithdrawalRequestsContractCall { message }
        }
        AlloyBlockValidationError::ConsolidationRequestsContractCall { message } => {
            BlockValidationError::ConsolidationRequestsContractCall { message }
        }
        AlloyBlockValidationError::DepositRequestDecode(message) => {
            BlockValidationError::DepositRequestDecode(message)
        }
        AlloyBlockValidationError::BlockGasExceeded => BlockValidationError::BlockGasExceeded,
        AlloyBlockValidationError::Other(error) => BlockValidationError::Other(error),
    }
}

fn convert_alloy_internal_block_execution_error(
    error: AlloyInternalBlockExecutionError,
) -> InternalBlockExecutionError {
    match error {
        AlloyInternalBlockExecutionError::EVM { hash, error } => {
            InternalBlockExecutionError::EVM { hash, error }
        }
        AlloyInternalBlockExecutionError::Other(error) => InternalBlockExecutionError::Other(error),
    }
}

pub(crate) fn prune_created_deleted_empty_accounts(bundle: &mut BundleState) {
    let Some(block_reverts) = bundle.reverts.last() else {
        return;
    };

    let delete_revert_accounts = block_reverts
        .iter()
        .filter_map(|(address, revert)| {
            matches!(revert.account, AccountInfoRevert::DeleteIt).then_some(*address)
        })
        .collect::<AddressHashSet>();

    if delete_revert_accounts.is_empty() {
        return;
    }

    let accounts = bundle
        .state
        .iter()
        .filter(|(address, account)| {
            delete_revert_accounts.contains(*address) &&
                account.original_info.is_none() &&
                account.info.is_none() &&
                (account.status == AccountStatus::InMemoryChange || account.was_destroyed()) &&
                account.storage.values().all(|slot| {
                    slot.previous_or_original_value.is_zero() && slot.present_value.is_zero()
                })
        })
        .map(|(&address, _)| address)
        .collect::<Vec<_>>();

    for &address in &accounts {
        if let Some(account) = bundle.state.remove(&address) {
            bundle.state_size = bundle.state_size.saturating_sub(account.size_hint());
        }
    }

    let accounts = accounts.into_iter().collect::<AddressHashSet>();
    let mut removed_reverts_size = 0;
    if let Some(block_reverts) = bundle.reverts.last_mut() {
        block_reverts.retain(|(address, revert)| {
            if accounts.contains(address) && matches!(revert.account, AccountInfoRevert::DeleteIt) {
                removed_reverts_size += revert.size_hint();
                return false;
            }
            true
        });
    }
    bundle.reverts_size = bundle.reverts_size.saturating_sub(removed_reverts_size);
}

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

        Ok(BlockExecutionOutput { state: state.take_bundle(), result: result? })
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

    /// Takes built [`BlockAccessList`] from executor.
    fn take_bal(&mut self) -> Option<BlockAccessList>;
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
    /// [`BundleState`] after the block execution.
    pub bundle_state: &'a BundleState,
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
        bundle_state: &'a BundleState,
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
    /// Block access list built during execution (EIP-7928, Amsterdam).
    pub block_access_list: Option<BlockAccessList>,
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

    /// Helper to access inner executor state mutably.
    fn db_mut(&mut self) -> &mut <Self::Executor as BlockExecutor>::DB {
        self.executor_mut().db_mut()
    }

    /// Helper to access inner executor state.
    fn db(&self) -> &<Self::Executor as BlockExecutor>::DB {
        self.executor().db()
    }

    /// Helper to access the builder block environment.
    fn block_env(&self) -> &<<Self::Executor as BlockExecutor>::Evm as Evm>::BlockEnv {
        self.executor().block_env()
    }

    /// Helper to access the builder configuration environment.
    fn cfg_env(&self) -> &CfgEnv<<<Self::Executor as BlockExecutor>::Evm as Evm>::Spec> {
        self.executor().cfg_env()
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
        DB = &'a mut State<DB>,
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
        BlockExecutor::apply_pre_execution_changes(&mut self.executor)?;
        BlockExecutor::db_mut(&mut self.executor).bump_bal_index();

        Ok(())
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutorTx<Self::Executor>,
        f: impl FnOnce(&<Self::Executor as BlockExecutor>::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        if let Some(gas_used) =
            self.executor.execute_transaction_with_commit_condition((tx_env, &tx), f)?
        {
            self.transactions.push(tx);
            BlockExecutor::db_mut(&mut self.executor).bump_bal_index();
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
        let (evm, result) = BlockExecutor::finish(self.executor)?;
        let (db, evm_env) = evm.finish();

        // merge all transitions into bundle state
        db.merge_transitions(BundleRetention::Reverts);
        prune_created_deleted_empty_accounts(&mut db.bundle_state);

        let block_access_list = db.take_built_alloy_bal();
        let block_access_list_hash =
            block_access_list.as_ref().map(|bal| compute_block_access_list_hash(bal.as_slice()));

        let hashed_state = state.hashed_post_state(&db.bundle_state);
        let (state_root, trie_updates) = match state_root_precomputed {
            Some(precomputed) => precomputed,
            None => state
                .state_root_with_updates(hashed_state.clone())
                .map_err(BlockExecutionError::other)?,
        };

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
        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;

        let has_bal = block.header().block_access_list_hash().is_some();

        if has_bal {
            BlockExecutor::db_mut(&mut executor).bal_state.bal_builder = Some(Bal::new());
        } else {
            BlockExecutor::db_mut(&mut executor).bal_state.bal_builder = None;
        }

        BlockExecutor::apply_pre_execution_changes(&mut executor)?;

        if has_bal {
            BlockExecutor::db_mut(&mut executor).bump_bal_index();
        }

        for tx in block.transactions_recovered() {
            BlockExecutor::execute_transaction(&mut executor, tx)?;
            if has_bal {
                BlockExecutor::db_mut(&mut executor).bump_bal_index();
            }
        }

        let result = BlockExecutor::apply_post_execution_changes(executor)?;

        self.db.merge_transitions(BundleRetention::Reverts);
        prune_created_deleted_empty_accounts(&mut self.db.bundle_state);

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
        let mut executor = self
            .strategy_factory
            .executor_for_block(&mut self.db, block)
            .map_err(BlockExecutionError::other)?;

        BlockExecutor::db_mut(&mut executor).set_reth_state_hook(Some(Box::new(state_hook)));

        let result = BlockExecutor::execute_block(executor, block.transactions_recovered());

        self.db.set_reth_state_hook(None);
        self.db.merge_transitions(BundleRetention::Reverts);
        prune_created_deleted_empty_accounts(&mut self.db.bundle_state);

        result
    }

    fn into_state(self) -> State<DB> {
        self.db
    }

    fn size_hint(&self) -> usize {
        self.db.bundle_state.size_hint()
    }

    fn take_bal(&mut self) -> Option<BlockAccessList> {
        self.db.take_built_alloy_bal()
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
    use crate::EmptyDB;
    use core::marker::PhantomData;
    use reth_ethereum_primitives::EthPrimitives;

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

        fn take_bal(&mut self) -> Option<BlockAccessList> {
            None
        }
    }

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = EmptyDB::default();
        let executor = provider.executor(db);
        let _ = executor.execute(&Default::default());
    }
}
