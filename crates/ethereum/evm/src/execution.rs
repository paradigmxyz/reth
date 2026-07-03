//! EVM-backed Ethereum execution helpers.

#[cfg(test)]
use crate::executor::HashedStateMode;
#[cfg(test)]
use crate::{convert::recovered_tx_envelope, RethReceiptBuilder};
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
#[cfg(test)]
use alloy_consensus::transaction::Recovered;
#[cfg(test)]
use alloy_consensus::TxType;
use alloy_consensus::{constants::ETH_TO_WEI, BlockHeader, Header};
#[cfg(test)]
use alloy_eips::eip2718::Typed2718;
use alloy_eips::{
    eip4895::Withdrawal,
    eip6110::{DEPOSIT_REQUEST_TYPE, MAINNET_DEPOSIT_CONTRACT_ADDRESS},
    eip7002::WITHDRAWAL_REQUEST_TYPE,
    eip7251::CONSOLIDATION_REQUEST_TYPE,
    eip7685::Requests,
};
use alloy_primitives::{map::AddressMap, Address, Bytes, B256, KECCAK256_EMPTY, U256};
use alloy_sol_types::{sol, SolEvent};
use core::{any::Any, convert::Infallible};
#[cfg(test)]
use evm2::evm::Db;
#[cfg(test)]
use evm2::Precompiles;
use evm2::{
    bytecode::Bytecode as ExecutableBytecode,
    ethereum::RecoveredTxEnvelope,
    evm::{
        AccountChange, AccountChangeRef, AccountInfo, BlockStateAccumulator, StateChangeSink,
        StateChangeSource, StateChanges, StorageChange, SystemTx, BEACON_ROOTS_ADDRESS,
        CONSOLIDATION_REQUEST_ADDRESS, HISTORY_STORAGE_ADDRESS, WITHDRAWAL_REQUEST_ADDRESS,
    },
    registry::HandlerError,
    BaseEvmTypes, ErrorCode, Evm, SpecId, TxResult,
};
#[cfg(test)]
use evm2::{
    env::BlockEnv,
    ethereum::ethereum_tx_registry,
    evm::{precompile::PrecompileProvider, Database, DynDatabase},
    ExecutionConfig, Version,
};
use reth_ethereum_primitives::Receipt;
#[cfg(test)]
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::execute::{
    BlockExecutionError, BlockValidationError, CommitChanges, EvmError, InvalidTxError,
};
#[cfg(test)]
use reth_execution_types::BlockExecutionOutput;
use reth_execution_types::HashedPostStateSink;
use reth_trie_common::{HashedPostState, KeccakKeyHasher};

const DEPOSIT_BYTES_SIZE: usize = 48 + 32 + 8 + 96 + 8;

sol! {
    #[allow(missing_docs)]
    event DepositEvent(
        bytes pubkey,
        bytes withdrawal_credentials,
        bytes amount,
        bytes signature,
        bytes index
    );
}

/// Error returned by EVM-backed Ethereum execution.
#[derive(Debug)]
pub enum EthExecutionError<E = DynamicDatabaseError> {
    /// EVM rejected the transaction during validation.
    InvalidTx(EthInvalidTxError),
    /// EVM rejected or halted transaction execution before producing a Reth output.
    Handler(HandlerError),
    /// EVM reported a database error and the typed database error was available.
    Database(E),
    /// EVM reported a database error, but the typed database error was no longer available.
    MissingDatabaseError(ErrorCode),
    /// Cancun requires a parent beacon block root after genesis.
    MissingParentBeaconBlockRoot,
    /// Cancun genesis payloads must carry a zero parent beacon block root.
    CancunGenesisParentBeaconBlockRootNotZero(B256),
    /// A pre-block system call reverted or halted without producing a successful result.
    SystemCallFailed {
        /// System contract address that was called.
        address: Address,
        /// EVM stop reason for the failed call.
        reason: String,
    },
    /// Deposit request logs could not be decoded.
    DepositRequestDecode(String),
}

/// Database error returned through evm2's dynamic database interface.
#[derive(Debug)]
pub struct DynamicDatabaseError(String);

impl DynamicDatabaseError {
    fn new(error: impl core::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl core::fmt::Display for DynamicDatabaseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for DynamicDatabaseError {}

impl<E> core::fmt::Display for EthExecutionError<E>
where
    E: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidTx(err) => write!(f, "invalid transaction: {err}"),
            Self::Handler(err) => write!(f, "EVM execution error: {err}"),
            Self::Database(err) => write!(f, "EVM database error: {err}"),
            Self::MissingDatabaseError(code) => {
                write!(f, "EVM database error {code:?} was not available")
            }
            Self::MissingParentBeaconBlockRoot => {
                f.write_str("missing parent beacon block root for Cancun system call")
            }
            Self::CancunGenesisParentBeaconBlockRootNotZero(root) => {
                write!(f, "Cancun genesis parent beacon block root must be zero, got {root}")
            }
            Self::SystemCallFailed { address, reason } => {
                write!(f, "EVM system call to {address} failed: {reason}")
            }
            Self::DepositRequestDecode(err) => write!(f, "failed to decode deposit request: {err}"),
        }
    }
}

impl<E> core::error::Error for EthExecutionError<E> where E: core::error::Error + Send + 'static {}

/// Ethereum transaction validation error returned by evm2 handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthInvalidTxError(HandlerError);

impl EthInvalidTxError {
    /// Returns the underlying evm2 handler error.
    pub fn handler_error(&self) -> HandlerError {
        self.0.clone()
    }
}

impl core::fmt::Display for EthInvalidTxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl core::error::Error for EthInvalidTxError {}

impl InvalidTxError for EthInvalidTxError {
    fn is_nonce_too_low(&self) -> bool {
        matches!(self.0, HandlerError::InvalidNonce { expected, got } if got < expected)
    }

    fn is_gas_limit_too_high(&self) -> bool {
        matches!(
            self.0,
            HandlerError::GasLimitMoreThanBlock { .. } |
                HandlerError::TxGasLimitGreaterThanCap { .. }
        )
    }

    fn is_gas_limit_too_low(&self) -> bool {
        matches!(self.0, HandlerError::IntrinsicGasTooLow { .. })
    }

    fn as_any(&self) -> &(dyn Any + 'static) {
        self
    }
}

impl<E> EvmError for EthExecutionError<E>
where
    E: core::error::Error + Send + Sync + 'static,
{
    type InvalidTransaction = EthInvalidTxError;

    fn as_invalid_tx_err(&self) -> Option<&<Self as EvmError>::InvalidTransaction> {
        match self {
            Self::InvalidTx(err) => Some(err),
            _ => None,
        }
    }

    fn try_into_invalid_tx_err(self) -> Result<<Self as EvmError>::InvalidTransaction, Self> {
        match self {
            Self::InvalidTx(err) => Ok(err),
            err => Err(err),
        }
    }

    fn is_fatal(&self) -> bool {
        self.as_invalid_tx_err().is_none()
    }
}

impl<E> From<EthExecutionError<E>> for BlockExecutionError
where
    E: core::error::Error + Send + Sync + 'static,
{
    fn from(err: EthExecutionError<E>) -> Self {
        match err {
            EthExecutionError::InvalidTx(err) => BlockValidationError::Other(Box::new(err)).into(),
            EthExecutionError::MissingParentBeaconBlockRoot => {
                BlockValidationError::MissingParentBeaconBlockRoot.into()
            }
            EthExecutionError::CancunGenesisParentBeaconBlockRootNotZero(
                parent_beacon_block_root,
            ) => BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                parent_beacon_block_root,
            }
            .into(),
            EthExecutionError::DepositRequestDecode(err) => {
                BlockValidationError::DepositRequestDecode(err).into()
            }
            err => Self::other(err),
        }
    }
}

const fn handler_error_is_invalid_tx(err: &HandlerError) -> bool {
    !matches!(err, HandlerError::Fatal(_) | HandlerError::WrongTransactionType { .. })
}

/// Error returned by payload execution over a fallible transaction stream.
#[derive(Debug)]
pub enum PayloadExecutionError<E, TxErr, ReceiptErr = Infallible> {
    /// The payload executor failed while executing the block.
    Execution(E),
    /// The transaction stream failed before yielding the next transaction.
    Transaction(TxErr),
    /// The receipt callback failed after a transaction committed.
    Receipt(ReceiptErr),
}

impl<E, TxErr, ReceiptErr> From<E> for PayloadExecutionError<E, TxErr, ReceiptErr> {
    fn from(err: E) -> Self {
        Self::Execution(err)
    }
}

impl<E, TxErr, ReceiptErr> core::fmt::Display for PayloadExecutionError<E, TxErr, ReceiptErr>
where
    E: core::fmt::Display,
    TxErr: core::fmt::Display,
    ReceiptErr: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Execution(err) => write!(f, "payload execution error: {err}"),
            Self::Transaction(err) => write!(f, "transaction stream error: {err}"),
            Self::Receipt(err) => write!(f, "receipt callback error: {err}"),
        }
    }
}

impl<E, TxErr, ReceiptErr> core::error::Error for PayloadExecutionError<E, TxErr, ReceiptErr>
where
    E: core::error::Error + Send + Sync + 'static,
    TxErr: core::error::Error + Send + Sync + 'static,
    ReceiptErr: core::error::Error + Send + Sync + 'static,
{
}

/// Additional block-level execution context.
#[derive(Debug, Clone, Copy)]
pub struct BlockExecutionContext<'a> {
    /// Chain id used for transaction validation and the `CHAINID` opcode.
    pub chain_id: u64,
    /// Pre-block system calls to run before transaction execution.
    pub system_calls: Option<BlockSystemCalls>,
    /// Pre-merge ommer headers included in the block.
    pub ommers: Option<&'a [Header]>,
    /// Post-block withdrawals to apply after transaction execution.
    pub withdrawals: Option<&'a [Withdrawal]>,
    /// Deposit contract address used to derive EIP-6110 deposit requests from receipts.
    pub deposit_contract_address: Option<Address>,
}

impl Default for BlockExecutionContext<'_> {
    fn default() -> Self {
        Self {
            chain_id: 1,
            system_calls: None,
            ommers: None,
            withdrawals: None,
            deposit_contract_address: None,
        }
    }
}

/// Inputs required by Ethereum pre-block system calls.
#[derive(Debug, Clone, Copy)]
pub struct BlockSystemCalls {
    /// Parent block hash for EIP-2935 history storage.
    pub parent_hash: B256,
    /// Parent beacon block root for EIP-4788 beacon roots.
    pub parent_beacon_block_root: Option<B256>,
}

/// Inputs required to execute an Ethereum block with evm2.
#[cfg(test)]
pub(crate) struct BlockExecutionInput<'a, DB> {
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    context: BlockExecutionContext<'a>,
    precompiles: Box<dyn PrecompileProvider<BaseEvmTypes>>,
}

#[cfg(test)]
impl<'a, DB> BlockExecutionInput<'a, DB> {
    /// Creates a new Ethereum block execution input.
    pub(crate) fn new(
        spec_id: SpecId,
        block_env: BlockEnv,
        database: DB,
        block_number: u64,
        context: BlockExecutionContext<'a>,
        precompiles: Box<dyn PrecompileProvider<BaseEvmTypes>>,
    ) -> Self {
        Self { spec_id, block_env, database, block_number, context, precompiles }
    }
}

#[cfg(test)]
impl<DB> BlockExecutionInput<'_, DB>
where
    DB: DynDatabase,
{
    /// Executes recovered Ethereum transactions.
    pub(crate) fn execute_recovered_transactions(
        self,
        transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    ) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError> {
        match self.execute_fallible_envelopes::<Infallible, Infallible, _, _, _, _>(
            transactions.into_iter().map(recovered_tx_envelope).map(Ok::<_, Infallible>),
            ExecutionHooks::new(|_| {}, ignore_receipt, |_| {}, HashedStateMode::OutputOnly),
        ) {
            Ok(output) => Ok(output),
            Err(PayloadExecutionError::Execution(err)) => Err(err),
            Err(PayloadExecutionError::Transaction(err) | PayloadExecutionError::Receipt(err)) => {
                match err {}
            }
        }
    }

    /// Executes a fallible stream of EVM-native recovered transaction envelopes.
    ///
    /// This consumes each transaction only when execution reaches it, so upstream transaction
    /// conversion can continue in parallel with earlier transaction execution.
    pub(crate) fn execute_fallible_envelopes<TxErr, ReceiptErr, I, F, R, H>(
        self,
        transactions: I,
        hooks: ExecutionHooks<F, R, H>,
    ) -> Result<
        BlockExecutionOutput<Receipt>,
        PayloadExecutionError<EthExecutionError, TxErr, ReceiptErr>,
    >
    where
        I: IntoIterator<Item = Result<RecoveredTxEnvelope, TxErr>>,
        F: FnMut(usize),
        R: for<'receipt> FnMut(usize, &'receipt Receipt) -> Result<(), ReceiptErr>,
        H: FnMut(HashedPostState),
    {
        let Self { spec_id, block_env, database, block_number, context, precompiles } = self;
        let ExecutionHooks {
            mut on_transaction_executed,
            mut on_receipt,
            mut on_hashed_state_update,
            hashed_state_mode,
        } = hooks;

        let block_beneficiary = block_env.beneficiary;
        let mut version = Version::new(spec_id);
        version.chain_id = context.chain_id;
        let mut evm = Evm::<BaseEvmTypes>::new_with_execution_config(
            ExecutionConfig::for_spec_and_version(spec_id, version),
            spec_id,
            block_env,
            ethereum_tx_registry(spec_id),
            database,
            precompiles,
        );
        let mut block_state = BlockStateAccumulator::new();
        let mut hashed_state =
            hashed_state_mode.output().then(HashedPostStateSink::<KeccakKeyHasher>::default);
        pre_execution_system_call_state_changes(
            &mut evm,
            &mut block_state,
            hashed_state.as_mut(),
            hashed_state_mode.stream(),
            &mut on_hashed_state_update,
            spec_id,
            block_number,
            context,
        )?;
        let mut receipts = Vec::new();
        let mut cumulative_gas_used = 0;

        for (index, transaction) in transactions.into_iter().enumerate() {
            let transaction = transaction.map_err(PayloadExecutionError::Transaction)?;
            let tx_type =
                TxType::try_from(transaction.ty()).expect("transaction envelope has valid type");
            let outcome = execute_transaction(
                &mut evm,
                &mut block_state,
                hashed_state.as_mut(),
                hashed_state_mode.stream(),
                &mut on_hashed_state_update,
                &transaction,
            )?;
            cumulative_gas_used += outcome.tx_gas_used();
            let receipt = RethReceiptBuilder.build_receipt(tx_type, outcome, cumulative_gas_used);
            on_receipt(index, &receipt).map_err(PayloadExecutionError::Receipt)?;
            receipts.push(receipt);
            on_transaction_executed(index + 1);
        }

        let mut requests = block_requests_from_receipts(spec_id, context, &receipts)?;
        post_execution_system_call_state_changes(
            &mut evm,
            &mut block_state,
            hashed_state.as_mut(),
            hashed_state_mode.stream(),
            &mut on_hashed_state_update,
            spec_id,
            context,
            &mut requests,
        )?;

        post_block_balance_state_changes(
            &mut evm,
            &mut block_state,
            hashed_state.as_mut(),
            hashed_state_mode.stream(),
            &mut on_hashed_state_update,
            spec_id,
            block_number,
            block_beneficiary,
            context.ommers,
            context.withdrawals,
        )?;

        let mut output = RethReceiptBuilder
            .build_block_output_from_receipts_and_state_with_hashed_state(
                receipts,
                block_state,
                hashed_state.map(HashedPostStateSink::into_hashed_post_state),
            );
        output.result.requests = requests;

        Ok(output)
    }
}

/// Hooks invoked while executing an Ethereum block.
#[cfg(test)]
pub(crate) struct ExecutionHooks<F, R, H> {
    on_transaction_executed: F,
    on_receipt: R,
    on_hashed_state_update: H,
    hashed_state_mode: HashedStateMode,
}

#[cfg(test)]
impl<F, R, H> ExecutionHooks<F, R, H> {
    /// Creates execution hooks.
    pub(crate) const fn new(
        on_transaction_executed: F,
        on_receipt: R,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Self {
        Self { on_transaction_executed, on_receipt, on_hashed_state_update, hashed_state_mode }
    }
}

#[cfg(test)]
const fn ignore_receipt(_index: usize, _receipt: &Receipt) -> Result<(), Infallible> {
    Ok(())
}

/// Executes a block worth of recovered Ethereum transactions with the active EVM.
#[cfg(test)]
fn execute_block<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError>
where
    DB: Database,
{
    execute_block_with_withdrawals(spec_id, block_env, database, block_number, transactions, None)
}

/// Executes a block worth of recovered Ethereum transactions and post-block withdrawals with the
/// active EVM.
#[cfg(test)]
fn execute_block_with_withdrawals<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError>
where
    DB: Database,
{
    execute_block_with_context(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        BlockExecutionContext {
            chain_id: 1,
            system_calls: None,
            ommers: None,
            withdrawals,
            deposit_contract_address: None,
        },
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context.
#[cfg(test)]
fn execute_block_with_context<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError>
where
    DB: Database,
{
    execute_block_with_context_and_precompiles(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        context,
        Box::new(Precompiles::base(spec_id)),
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context
/// and the provided precompile provider.
#[cfg(test)]
fn execute_block_with_context_and_precompiles<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: BlockExecutionContext<'_>,
    precompiles: Box<dyn PrecompileProvider<BaseEvmTypes>>,
) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError>
where
    DB: Database,
{
    BlockExecutionInput::new(
        spec_id,
        block_env,
        Db::new(database),
        block_number,
        context,
        precompiles,
    )
    .execute_recovered_transactions(transactions)
}

fn map_handler_error(evm: &mut Evm<'_, BaseEvmTypes>, err: HandlerError) -> EthExecutionError {
    match err {
        HandlerError::Fatal(code) => map_db_error_code(evm, code),
        err if handler_error_is_invalid_tx(&err) => {
            EthExecutionError::InvalidTx(EthInvalidTxError(err))
        }
        err => EthExecutionError::Handler(err),
    }
}

#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) fn apply_pre_execution_system_calls(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_number: u64,
    context: BlockExecutionContext<'_>,
) -> Result<BlockStateAccumulator, EthExecutionError> {
    let mut block_state = BlockStateAccumulator::new();
    let spec_id = evm.spec_id();
    pre_execution_system_call_state_changes(
        evm,
        &mut block_state,
        None,
        false,
        &mut |_| {},
        spec_id,
        block_number,
        context,
    )?;
    Ok(block_state)
}

fn take_database_error(evm: &mut Evm<'_, BaseEvmTypes>, code: ErrorCode) -> DynamicDatabaseError {
    DynamicDatabaseError::new(evm.database_mut().error(code))
}

struct RethStateSink<'a> {
    execution_sink: Option<&'a mut dyn StateChangeSink<Error = Infallible>>,
    block_state: &'a mut BlockStateAccumulator,
    output_hashed_state: Option<&'a mut HashedPostStateSink<KeccakKeyHasher>>,
    streamed_hashed_state: Option<HashedPostStateSink<KeccakKeyHasher>>,
}

impl<'a> RethStateSink<'a> {
    fn new(
        execution_sink: Option<&'a mut dyn StateChangeSink<Error = Infallible>>,
        block_state: &'a mut BlockStateAccumulator,
        output_hashed_state: Option<&'a mut HashedPostStateSink<KeccakKeyHasher>>,
        stream_hashed_state: bool,
    ) -> Self {
        Self {
            execution_sink,
            block_state,
            output_hashed_state,
            streamed_hashed_state: stream_hashed_state
                .then(HashedPostStateSink::<KeccakKeyHasher>::default),
        }
    }

    fn flush_streamed_hashed_state(self, on_hashed_state_update: &mut impl FnMut(HashedPostState)) {
        if let Some(streamed_hashed_state) = self.streamed_hashed_state {
            send_hashed_state_update(
                streamed_hashed_state.into_hashed_post_state(),
                on_hashed_state_update,
            );
        }
    }
}

impl StateChangeSink for RethStateSink<'_> {
    type Error = Infallible;

    fn bytecode(&mut self, code_hash: B256, code: &ExecutableBytecode) -> Result<(), Self::Error> {
        if let Some(execution_sink) = self.execution_sink.as_deref_mut() {
            execution_sink.bytecode(code_hash, code)?;
        }
        self.block_state.bytecode(code_hash, code)?;
        if let Some(output_hashed_state) = self.output_hashed_state.as_deref_mut() {
            output_hashed_state.bytecode(code_hash, code)?;
        }
        if let Some(streamed_hashed_state) = self.streamed_hashed_state.as_mut() {
            streamed_hashed_state.bytecode(code_hash, code)?;
        }
        Ok(())
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        if let Some(execution_sink) = self.execution_sink.as_deref_mut() {
            execution_sink.account(change)?;
        }
        self.block_state.account(change)?;
        if let Some(output_hashed_state) = self.output_hashed_state.as_deref_mut() {
            output_hashed_state.account(change)?;
        }
        if let Some(streamed_hashed_state) = self.streamed_hashed_state.as_mut() {
            streamed_hashed_state.account(change)?;
        }
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        if let Some(execution_sink) = self.execution_sink.as_deref_mut() {
            execution_sink.storage_wipe(address)?;
        }
        self.block_state.storage_wipe(address)?;
        if let Some(output_hashed_state) = self.output_hashed_state.as_deref_mut() {
            output_hashed_state.storage_wipe(address)?;
        }
        if let Some(streamed_hashed_state) = self.streamed_hashed_state.as_mut() {
            streamed_hashed_state.storage_wipe(address)?;
        }
        Ok(())
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        if let Some(execution_sink) = self.execution_sink.as_deref_mut() {
            execution_sink.storage(change)?;
        }
        self.block_state.storage(change)?;
        if let Some(output_hashed_state) = self.output_hashed_state.as_deref_mut() {
            output_hashed_state.storage(change)?;
        }
        if let Some(streamed_hashed_state) = self.streamed_hashed_state.as_mut() {
            streamed_hashed_state.storage(change)?;
        }
        Ok(())
    }
}

fn send_hashed_state_update(
    hashed_state: HashedPostState,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
) {
    if !hashed_state.is_empty() {
        on_hashed_state_update(hashed_state);
    }
}

#[cfg(test)]
pub(crate) fn execute_transaction(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    transaction: &RecoveredTxEnvelope,
) -> Result<TxResult, EthExecutionError> {
    execute_transaction_with_commit_condition(
        evm,
        block_state,
        hashed_state,
        stream_hashed_state,
        on_hashed_state_update,
        transaction,
        |_| CommitChanges::Yes,
    )
    .map(|outcome| outcome.expect("transaction is always committed"))
}

pub(crate) fn execute_transaction_with_commit_condition(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    transaction: &RecoveredTxEnvelope,
    should_commit: impl FnOnce(&TxResult) -> CommitChanges,
) -> Result<Option<TxResult>, EthExecutionError> {
    enum TransactionResolution {
        Outcome(Option<TxResult>),
        DatabaseError(ErrorCode),
        HandlerError(HandlerError),
    }

    let resolution = match evm.transact(transaction) {
        Ok(executed) => {
            if let Some(code) = executed.result().error_code {
                let _ = executed.discard();
                TransactionResolution::DatabaseError(code)
            } else if !should_commit(executed.result()).should_commit() {
                let _ = executed.discard();
                TransactionResolution::Outcome(None)
            } else {
                let outcome = {
                    let mut sink =
                        RethStateSink::new(None, block_state, hashed_state, stream_hashed_state);
                    let Ok(outcome) = executed.commit_with(&mut sink);
                    sink.flush_streamed_hashed_state(on_hashed_state_update);
                    outcome
                };
                TransactionResolution::Outcome(Some(outcome))
            }
        }
        Err(err) => TransactionResolution::HandlerError(err),
    };

    match resolution {
        TransactionResolution::Outcome(outcome) => Ok(outcome),
        TransactionResolution::DatabaseError(code) => Err(map_db_error_code(evm, code)),
        TransactionResolution::HandlerError(err) => Err(map_handler_error(evm, err)),
    }
}

fn map_db_error_code(evm: &mut Evm<'_, BaseEvmTypes>, code: ErrorCode) -> EthExecutionError {
    EthExecutionError::Database(take_database_error(evm, code))
}

#[expect(clippy::needless_option_as_deref, clippy::too_many_arguments)]
pub(crate) fn pre_execution_system_call_state_changes(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    spec_id: SpecId,
    block_number: u64,
    context: BlockExecutionContext<'_>,
) -> Result<(), EthExecutionError> {
    let Some(system_calls) = context.system_calls else {
        return Ok(());
    };
    let mut hashed_state = hashed_state;

    if spec_id.enables(SpecId::PRAGUE) && block_number != 0 {
        let _ = execute_system_call(
            evm,
            block_state,
            hashed_state.as_deref_mut(),
            stream_hashed_state,
            on_hashed_state_update,
            HISTORY_STORAGE_ADDRESS,
            system_calls.parent_hash.0.into(),
        )?;
    }

    if spec_id.enables(SpecId::CANCUN) {
        let parent_beacon_block_root = system_calls
            .parent_beacon_block_root
            .ok_or(EthExecutionError::MissingParentBeaconBlockRoot)?;

        if block_number == 0 {
            if parent_beacon_block_root != B256::ZERO {
                return Err(EthExecutionError::CancunGenesisParentBeaconBlockRootNotZero(
                    parent_beacon_block_root,
                ));
            }
        } else {
            let _ = execute_system_call(
                evm,
                block_state,
                hashed_state.as_deref_mut(),
                stream_hashed_state,
                on_hashed_state_update,
                BEACON_ROOTS_ADDRESS,
                parent_beacon_block_root.0.into(),
            )?;
        }
    }

    Ok(())
}

pub(crate) fn block_requests_from_receipts(
    spec_id: SpecId,
    context: BlockExecutionContext<'_>,
    receipts: &[Receipt],
) -> Result<Requests, EthExecutionError> {
    let mut requests = Requests::default();
    if context.system_calls.is_none() || !spec_id.enables(SpecId::PRAGUE) {
        return Ok(requests)
    }

    let deposit_requests = parse_deposit_requests_from_receipts(
        context.deposit_contract_address.unwrap_or(MAINNET_DEPOSIT_CONTRACT_ADDRESS),
        receipts,
    )?;
    requests.push_request_with_type(DEPOSIT_REQUEST_TYPE, deposit_requests);

    Ok(requests)
}

fn parse_deposit_requests_from_receipts(
    deposit_contract_address: Address,
    receipts: &[Receipt],
) -> Result<Vec<u8>, EthExecutionError> {
    let mut out = Vec::new();
    for receipt in receipts {
        for log in &receipt.logs {
            if log.address != deposit_contract_address ||
                log.topics().first() != Some(&DepositEvent::SIGNATURE_HASH)
            {
                continue
            }

            let decoded = DepositEvent::decode_log(log)
                .map_err(|err| EthExecutionError::DepositRequestDecode(err.to_string()))?;
            out.reserve(DEPOSIT_BYTES_SIZE);
            out.extend_from_slice(decoded.pubkey.as_ref());
            out.extend_from_slice(decoded.withdrawal_credentials.as_ref());
            out.extend_from_slice(decoded.amount.as_ref());
            out.extend_from_slice(decoded.signature.as_ref());
            out.extend_from_slice(decoded.index.as_ref());
        }
    }

    Ok(out)
}

#[expect(clippy::needless_option_as_deref, clippy::too_many_arguments)]
pub(crate) fn post_execution_system_call_state_changes(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    spec_id: SpecId,
    context: BlockExecutionContext<'_>,
    requests: &mut Requests,
) -> Result<(), EthExecutionError> {
    if context.system_calls.is_none() || !spec_id.enables(SpecId::PRAGUE) {
        return Ok(());
    }
    let mut hashed_state = hashed_state;

    let withdrawal_requests = execute_system_call(
        evm,
        block_state,
        hashed_state.as_deref_mut(),
        stream_hashed_state,
        on_hashed_state_update,
        WITHDRAWAL_REQUEST_ADDRESS,
        Bytes::new(),
    )?;
    requests.push_request_with_type(
        WITHDRAWAL_REQUEST_TYPE,
        withdrawal_requests.output.iter().copied(),
    );

    let consolidation_requests = execute_system_call(
        evm,
        block_state,
        hashed_state.as_deref_mut(),
        stream_hashed_state,
        on_hashed_state_update,
        CONSOLIDATION_REQUEST_ADDRESS,
        Bytes::new(),
    )?;
    requests.push_request_with_type(
        CONSOLIDATION_REQUEST_TYPE,
        consolidation_requests.output.iter().copied(),
    );

    Ok(())
}

fn execute_system_call(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    address: Address,
    data: Bytes,
) -> Result<TxResult, EthExecutionError> {
    enum SystemCallResolution {
        Outcome(TxResult),
        DatabaseError(ErrorCode),
        HandlerError(HandlerError),
        Failed(String),
    }

    let resolution = match evm.system_call(SystemTx::new(address, data)) {
        Ok(executed) => {
            if let Some(code) = executed.result().error_code {
                let _ = executed.discard();
                SystemCallResolution::DatabaseError(code)
            } else if !executed.result().status {
                let reason = format!("{:?}", executed.result().stop);
                let _ = executed.discard();
                SystemCallResolution::Failed(reason)
            } else {
                let outcome = {
                    let mut sink =
                        RethStateSink::new(None, block_state, hashed_state, stream_hashed_state);
                    let Ok(outcome) = executed.commit_with(&mut sink);
                    sink.flush_streamed_hashed_state(on_hashed_state_update);
                    outcome
                };
                SystemCallResolution::Outcome(outcome)
            }
        }
        Err(err) => SystemCallResolution::HandlerError(err),
    };

    match resolution {
        SystemCallResolution::Outcome(outcome) => Ok(outcome),
        SystemCallResolution::DatabaseError(code) => Err(map_db_error_code(evm, code)),
        SystemCallResolution::HandlerError(err) => Err(map_handler_error(evm, err)),
        SystemCallResolution::Failed(reason) => {
            Err(EthExecutionError::SystemCallFailed { address, reason })
        }
    }
}

fn commit_state_changes<S: StateChangeSource>(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    changes: &S,
) {
    let result = {
        let mut sink = RethStateSink::new(
            Some(evm.overlay_db_mut() as &mut dyn StateChangeSink<Error = Infallible>),
            block_state,
            hashed_state,
            stream_hashed_state,
        );
        let result = changes.visit(&mut sink);
        if result.is_ok() {
            sink.flush_streamed_hashed_state(on_hashed_state_update);
        }
        result
    };
    match result {
        Ok(()) => {}
        Err(err) => match err {},
    }
}

#[expect(clippy::too_many_arguments)]
pub(crate) fn post_block_balance_state_changes(
    evm: &mut Evm<'_, BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    hashed_state: Option<&mut HashedPostStateSink<KeccakKeyHasher>>,
    stream_hashed_state: bool,
    on_hashed_state_update: &mut impl FnMut(HashedPostState),
    spec_id: SpecId,
    block_number: u64,
    block_beneficiary: Address,
    ommers: Option<&[Header]>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<(), EthExecutionError> {
    let mut balance_increments = AddressMap::<U256>::default();

    if let Some(base_block_reward) = base_block_reward(spec_id) {
        let ommers = ommers.unwrap_or_default();
        for ommer in ommers {
            *balance_increments.entry(ommer.beneficiary()).or_default() +=
                U256::from(ommer_reward(base_block_reward, block_number, ommer.number()));
        }
        *balance_increments.entry(block_beneficiary).or_default() +=
            U256::from(block_reward(base_block_reward, ommers.len()));
    }

    for withdrawal in withdrawals.into_iter().flatten() {
        *balance_increments.entry(withdrawal.address).or_default() += withdrawal.amount_wei();
    }

    if balance_increments.is_empty() {
        return Ok(());
    }

    let mut changes = StateChanges::default();

    for (address, increment) in balance_increments {
        let original =
            evm.read_account_info(&address).map_err(|code| map_db_error_code(evm, code))?;
        let mut current = original.clone().unwrap_or_else(empty_account);

        current.balance = current.balance.saturating_add(increment);
        let mut change = AccountChange::default();
        change.original = original;
        change.current = Some(current);
        changes.accounts.insert(address, change);
    }

    commit_state_changes(
        evm,
        block_state,
        hashed_state,
        stream_hashed_state,
        on_hashed_state_update,
        &changes,
    );

    Ok(())
}

const fn base_block_reward(spec_id: SpecId) -> Option<u128> {
    if spec_id.enables(SpecId::MERGE) {
        None
    } else if spec_id.enables(SpecId::PETERSBURG) {
        Some(ETH_TO_WEI * 2)
    } else if spec_id.enables(SpecId::BYZANTIUM) {
        Some(ETH_TO_WEI * 3)
    } else {
        Some(ETH_TO_WEI * 5)
    }
}

const fn block_reward(base_block_reward: u128, ommers: usize) -> u128 {
    base_block_reward + (base_block_reward >> 5) * ommers as u128
}

fn ommer_reward(base_block_reward: u128, block_number: u64, ommer_block_number: u64) -> u128 {
    let distance = 8u64.saturating_add(ommer_block_number).saturating_sub(block_number);
    (u128::from(distance) * base_block_reward) >> 3
}

const fn empty_account() -> AccountInfo {
    AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: KECCAK256_EMPTY,
        code: None,
        _non_exhaustive: (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_eips::{
        eip2935::{HISTORY_SERVE_WINDOW, HISTORY_STORAGE_CODE},
        eip4788::BEACON_ROOTS_CODE,
        eip4895::Withdrawal,
        eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_CODE,
    };
    use alloy_primitives::{address, Address, Bytes, Log, Signature, TxKind, B256, U256};
    use core::convert::Infallible;
    use evm2::{
        bytecode::Bytecode,
        evm::AccountInfo,
        interpreter::{opcode::op, Word},
    };
    use reth_execution_types::hashed_post_state_from_execution_state;

    fn assert_hashed_state_matches_output(output: &BlockExecutionOutput<Receipt>) {
        let hashed_state = output.hashed_state.clone().expect("EVM execution hashes post-state");
        let recomputed =
            hashed_post_state_from_execution_state::<KeccakKeyHasher>(output.state.inner());

        assert_eq!(hashed_state.into_sorted(), recomputed.into_sorted());
    }

    fn assert_hashed_state_matches_streamed_updates(
        output: &BlockExecutionOutput<Receipt>,
        updates: Vec<HashedPostState>,
    ) {
        let mut streamed = HashedPostState::default();
        for update in updates {
            streamed.extend(update);
        }
        let recomputed =
            hashed_post_state_from_execution_state::<KeccakKeyHasher>(output.state.inner());

        assert_eq!(streamed.into_sorted(), recomputed.into_sorted());
    }

    fn legacy_transfer(
        caller: Address,
        target: Address,
        value: U256,
    ) -> Recovered<TransactionSigned> {
        Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1,
                    gas_limit: 21_000,
                    to: TxKind::Call(target),
                    value,
                    input: Bytes::new(),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        )
    }

    #[derive(Default)]
    struct TestDatabase {
        accounts: BTreeMap<Address, AccountInfo>,
        storage: BTreeMap<(Address, Word), Word>,
    }

    impl Database for TestDatabase {
        type Error = Infallible;

        fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(self.accounts.get(address).cloned())
        }

        fn get_code_by_hash(&mut self, _code_hash: &B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
            Ok(self.storage.get(&(*address, *key)).copied().unwrap_or_default())
        }

        fn get_block_hash(&mut self, _number: &Word) -> Result<Option<B256>, Self::Error> {
            Ok(None)
        }
    }

    #[derive(Debug)]
    struct TestTxError;

    impl core::fmt::Display for TestTxError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("test transaction stream error")
        }
    }

    impl core::error::Error for TestTxError {}

    #[test]
    fn executes_legacy_transfer() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = legacy_transfer(caller, target, U256::from(1));

        let output =
            execute_block(SpecId::FRONTIER, BlockEnv::default(), database, 1, [transaction])
                .expect("EVM execution succeeds");

        assert_eq!(output.result.gas_used, 21_000);
        assert_eq!(output.result.receipts.len(), 1);
        assert!(output.result.receipts[0].success);
        assert_eq!(
            output.account_state(&target).unwrap().current.as_ref().unwrap().balance,
            U256::from(1)
        );
    }

    #[test]
    fn fallible_transaction_stream_is_consumed_lazily() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = legacy_transfer(caller, target, U256::from(1));

        let mut executed = 0;
        let result = BlockExecutionInput::new(
            SpecId::FRONTIER,
            BlockEnv::default(),
            Db::new(database),
            1,
            BlockExecutionContext::default(),
            Box::new(Precompiles::base(SpecId::FRONTIER)),
        )
        .execute_fallible_envelopes::<TestTxError, Infallible, _, _, _, _>(
            [Ok(recovered_tx_envelope(transaction)), Err(TestTxError)],
            ExecutionHooks::new(
                |count| executed = count,
                ignore_receipt,
                |_| {},
                HashedStateMode::OutputOnly,
            ),
        );

        assert_eq!(executed, 1);
        assert!(matches!(result, Err(PayloadExecutionError::Transaction(TestTxError))));
    }

    #[test]
    fn receipt_callback_streams_ordered_transaction_receipts() {
        let caller = address!("0000000000000000000000000000000000000001");
        let other = address!("0000000000000000000000000000000000000002");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        database
            .accounts
            .insert(other, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let first = legacy_transfer(caller, target, U256::from(1));
        let second = legacy_transfer(other, target, U256::from(2));

        let mut streamed_receipts = Vec::new();
        let output = BlockExecutionInput::new(
            SpecId::FRONTIER,
            BlockEnv::default(),
            Db::new(database),
            1,
            BlockExecutionContext::default(),
            Box::new(Precompiles::base(SpecId::FRONTIER)),
        )
        .execute_fallible_envelopes::<Infallible, Infallible, _, _, _, _>(
            [Ok(recovered_tx_envelope(first)), Ok(recovered_tx_envelope(second))],
            ExecutionHooks::new(
                |_| {},
                |index, receipt: &Receipt| {
                    streamed_receipts.push((index, receipt.clone()));
                    Ok::<(), Infallible>(())
                },
                |_| {},
                HashedStateMode::OutputOnly,
            ),
        )
        .expect("EVM execution succeeds");

        assert_eq!(streamed_receipts.len(), 2);
        assert_eq!(streamed_receipts[0].0, 0);
        assert_eq!(streamed_receipts[1].0, 1);
        assert_eq!(streamed_receipts[0].1.cumulative_gas_used, 21_000);
        assert_eq!(streamed_receipts[1].1.cumulative_gas_used, 42_000);
        assert_eq!(
            streamed_receipts.into_iter().map(|(_, receipt)| receipt).collect::<Vec<_>>(),
            output.result.receipts
        );
    }

    #[test]
    fn streams_hashed_state_without_output_hashed_state() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = legacy_transfer(caller, target, U256::from(1));

        let mut streamed_updates = Vec::new();
        let output = BlockExecutionInput::new(
            SpecId::FRONTIER,
            BlockEnv::default(),
            Db::new(database),
            1,
            BlockExecutionContext::default(),
            Box::new(Precompiles::base(SpecId::FRONTIER)),
        )
        .execute_fallible_envelopes::<Infallible, Infallible, _, _, _, _>(
            [Ok(recovered_tx_envelope(transaction))],
            ExecutionHooks::new(
                |_| {},
                ignore_receipt,
                |update| streamed_updates.push(update),
                HashedStateMode::StreamOnly,
            ),
        )
        .expect("EVM execution succeeds");

        assert!(output.hashed_state.is_none());
        assert!(!streamed_updates.is_empty());
        assert_hashed_state_matches_streamed_updates(&output, streamed_updates);
    }

    #[test]
    fn output_only_hashed_state_does_not_stream_updates() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = legacy_transfer(caller, target, U256::from(1));

        let mut streamed_updates = Vec::new();
        let output = BlockExecutionInput::new(
            SpecId::FRONTIER,
            BlockEnv::default(),
            Db::new(database),
            1,
            BlockExecutionContext::default(),
            Box::new(Precompiles::base(SpecId::FRONTIER)),
        )
        .execute_fallible_envelopes::<Infallible, Infallible, _, _, _, _>(
            [Ok(recovered_tx_envelope(transaction))],
            ExecutionHooks::new(
                |_| {},
                ignore_receipt,
                |update| streamed_updates.push(update),
                HashedStateMode::OutputOnly,
            ),
        )
        .expect("EVM execution succeeds");

        assert!(streamed_updates.is_empty());
        assert_hashed_state_matches_output(&output);
    }

    #[test]
    fn charges_london_sstore_set_gas() {
        let caller = address!("0000000000000000000000000000000000000001");
        let contract = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)));
        database.accounts.insert(
            contract,
            AccountInfo::default().with_nonce(1).with_code(Bytecode::new_raw(Bytes::from(vec![
                op::PUSH1,
                10,
                op::PUSH1,
                0,
                op::SSTORE,
                op::STOP,
            ]))),
        );
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1_000_000_000,
                    gas_limit: 100_000,
                    to: TxKind::Call(contract),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let output = execute_block(
            SpecId::LONDON,
            BlockEnv {
                gas_limit: U256::from(1_500_000),
                basefee: U256::from(1),
                ..Default::default()
            },
            database,
            1,
            [transaction],
        )
        .expect("EVM execution succeeds");

        assert_eq!(output.result.receipts[0].cumulative_gas_used, 43_106);
        assert_eq!(output.storage(&contract, U256::ZERO).unwrap(), U256::from(10));
    }

    #[test]
    fn rejects_transaction_gas_limit_above_block_gas_limit() {
        let caller = address!("0000000000000000000000000000000000000001");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)));
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1,
                    gas_limit: 2_500_000,
                    to: TxKind::Call(Address::ZERO),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let block_env = BlockEnv { gas_limit: U256::from(1_500_000), ..Default::default() };
        let err = execute_block(SpecId::FRONTIER, block_env, database, 1, [transaction])
            .expect_err("transaction gas limit above block gas limit should fail");

        assert!(matches!(
            err,
            EthExecutionError::InvalidTx(err)
                if matches!(
                    err.handler_error(),
                    HandlerError::GasLimitMoreThanBlock { gas_limit, block_gas_limit }
                        if gas_limit == 2_500_000 &&
                            block_gas_limit == U256::from(1_500_000)
                )
        ));
    }

    #[test]
    fn applies_withdrawals_to_block_output() {
        let existing = address!("0000000000000000000000000000000000000001");
        let new = address!("0000000000000000000000000000000000000002");
        let mut database = TestDatabase::default();
        database.accounts.insert(existing, AccountInfo::default().with_balance(U256::from(100)));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: existing, amount: 1 },
            Withdrawal { index: 1, validator_index: 1, address: new, amount: 2 },
            Withdrawal { index: 2, validator_index: 2, address: new, amount: 3 },
        ];

        let output = execute_block_with_withdrawals(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("EVM execution succeeds");

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.account_state(&existing).unwrap().current.as_ref().unwrap().balance,
            U256::from(1_000_000_100)
        );
        assert_eq!(
            output.account_state(&new).unwrap().current.as_ref().unwrap().balance,
            U256::from(5_000_000_000u64)
        );
        assert_hashed_state_matches_output(&output);
    }

    #[test]
    fn rejects_missing_cancun_parent_beacon_root() {
        let err = execute_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: None,
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect_err("missing parent beacon block root should fail");

        assert!(matches!(err, EthExecutionError::MissingParentBeaconBlockRoot));
    }

    #[test]
    fn rejects_nonzero_cancun_genesis_parent_beacon_root() {
        let root = B256::from([1u8; 32]);
        let err = execute_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            0,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(root),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect_err("nonzero Cancun genesis parent beacon block root should fail");

        assert!(matches!(
            err,
            EthExecutionError::CancunGenesisParentBeaconBlockRootNotZero(actual) if actual == root
        ));
    }

    #[test]
    fn writes_beacon_root_contract_storage() {
        let mut database = TestDatabase::default();
        database.accounts.insert(
            BEACON_ROOTS_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(BEACON_ROOTS_CODE.clone())),
        );

        let timestamp = U256::from(1);
        let parent_beacon_block_root = B256::with_last_byte(0x69);
        let output = execute_block_with_context(
            SpecId::CANCUN,
            BlockEnv { number: U256::from(1), timestamp, ..Default::default() },
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(parent_beacon_block_root),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect("beacon roots system call succeeds");

        let timestamp_index = timestamp % U256::from(HISTORY_SERVE_WINDOW);
        let root_index = timestamp_index + U256::from(HISTORY_SERVE_WINDOW);
        assert_eq!(output.storage(&BEACON_ROOTS_ADDRESS, timestamp_index).unwrap(), timestamp);
        assert_eq!(
            output.storage(&BEACON_ROOTS_ADDRESS, root_index).unwrap(),
            U256::from_be_bytes(parent_beacon_block_root.0)
        );
        assert_hashed_state_matches_output(&output);
    }

    #[test]
    fn writes_parent_hash_history_storage() {
        let mut database = TestDatabase::default();
        database.accounts.insert(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
        );

        let parent_hash = B256::with_last_byte(0x42);
        let output = execute_block_with_context(
            SpecId::PRAGUE,
            BlockEnv { number: U256::from(1), ..Default::default() },
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect("history storage system call succeeds");

        assert_eq!(
            output.storage(&HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::from_be_bytes(parent_hash.0)
        );
        assert_hashed_state_matches_output(&output);
    }

    #[test]
    fn runs_pre_execution_system_calls_without_receipts() {
        let output = execute_block_with_context(
            SpecId::PRAGUE,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::from([2u8; 32]),
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect("system calls to absent contracts are no-ops");

        assert!(output.result.receipts.is_empty());
        assert_eq!(output.state.accounts().count(), 0);
    }

    #[test]
    fn collects_post_execution_system_call_requests() {
        let mut database = TestDatabase::default();
        database.accounts.insert(
            WITHDRAWAL_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xaa)),
        );
        database.accounts.insert(
            CONSOLIDATION_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xbb)),
        );

        let output = execute_block_with_context(
            SpecId::PRAGUE,
            BlockEnv::default(),
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect("system calls succeed");

        assert_eq!(
            output.result.requests.iter().cloned().collect::<Vec<_>>(),
            vec![
                Bytes::from_static(&[WITHDRAWAL_REQUEST_TYPE, 0xaa]),
                Bytes::from_static(&[CONSOLIDATION_REQUEST_TYPE, 0xbb]),
            ]
        );
        assert!(output.result.receipts.is_empty());
        assert_hashed_state_matches_output(&output);
    }

    #[test]
    fn collects_deposit_requests_from_transaction_logs() {
        let deposit = DepositEvent {
            pubkey: Bytes::from(vec![0x11; 48]),
            withdrawal_credentials: Bytes::from(vec![0x22; 32]),
            amount: Bytes::from(vec![0x33; 8]),
            signature: Bytes::from(vec![0x44; 96]),
            index: Bytes::from(vec![0x55; 8]),
        };
        let log = DepositEvent::encode_log(&Log {
            address: MAINNET_DEPOSIT_CONTRACT_ADDRESS,
            data: deposit,
        });
        let receipt = Receipt { tx_type: TxType::Legacy, logs: vec![log], ..Default::default() };

        let requests = block_requests_from_receipts(
            SpecId::PRAGUE,
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
            &[receipt],
        )
        .expect("deposit log decodes");

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][0], DEPOSIT_REQUEST_TYPE);
        assert_eq!(requests[0].len(), 1 + DEPOSIT_BYTES_SIZE);
    }

    #[test]
    fn executes_withdrawal_request_contract() {
        let caller = address!("0000000000000000000000000000000000000001");
        let mut database = TestDatabase::default();
        database.accounts.insert(
            caller,
            AccountInfo::default().with_nonce(1).with_balance(U256::from(ETH_TO_WEI)),
        );
        database.accounts.insert(
            WITHDRAWAL_REQUEST_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone())),
        );

        let validator_public_key = [0x11; 48];
        let withdrawal_amount = [0x22; 8];
        let input =
            Bytes::from([validator_public_key.as_slice(), withdrawal_amount.as_slice()].concat());
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    nonce: 1,
                    gas_price: 1,
                    gas_limit: 135_856,
                    to: TxKind::Call(WITHDRAWAL_REQUEST_ADDRESS),
                    value: U256::from(2),
                    input,
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let output = execute_block_with_context(
            SpecId::PRAGUE,
            BlockEnv { gas_limit: U256::from(1_500_000), ..Default::default() },
            database,
            1,
            [transaction],
            BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
                deposit_contract_address: None,
            },
        )
        .expect("withdrawal request transaction succeeds");

        assert!(output.result.receipts.first().unwrap().success);
        assert_eq!(output.result.requests.len(), 1);
        assert_eq!(output.result.requests[0][0], WITHDRAWAL_REQUEST_TYPE);
        assert_hashed_state_matches_output(&output);
    }

    fn return_byte_code(value: u8) -> Bytecode {
        Bytecode::new_legacy(Bytes::from(vec![
            op::PUSH1,
            value,
            op::PUSH1,
            0,
            op::MSTORE8,
            op::PUSH1,
            1,
            op::PUSH1,
            0,
            op::RETURN,
        ]))
    }
}
