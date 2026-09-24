//! EVM-backed Ethereum execution helpers.
use reth_execution_types::{BlockState, EvmState, TransactionChanges};

use alloy_evm::eth::dao_fork;

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use alloy_consensus::{
    constants::ETH_TO_WEI, transaction::Recovered, BlockHeader, Header, TxReceipt,
};
use alloy_eips::{
    eip2718::Typed2718,
    eip4895::Withdrawal,
    eip6110::{DEPOSIT_REQUEST_TYPE, MAINNET_DEPOSIT_CONTRACT_ADDRESS},
    eip7002::WITHDRAWAL_REQUEST_TYPE,
    eip7251::CONSOLIDATION_REQUEST_TYPE,
    eip7685::Requests,
};
use alloy_primitives::{map::AddressMap, Address, Bytes, Log, B256, U256};
use alloy_sol_types::{sol, SolEvent};
use core::any::Any;
use evm2::{
    evm::{
        AccountChangeRef, AccountInfo, StateChangeSink, StateChangeSource, SystemTx,
        BEACON_ROOTS_ADDRESS, BUILDER_DEPOSIT_REQUEST_ADDRESS, BUILDER_EXIT_REQUEST_ADDRESS,
        CONSOLIDATION_REQUEST_ADDRESS, HISTORY_STORAGE_ADDRESS, WITHDRAWAL_REQUEST_ADDRESS,
    },
    registry::HandlerError,
    ErrorCode, Evm, EvmTypes, SpecId, TxResult, TxResultWithState,
};
use reth_ethereum_forks::EthereumHardforks;
use reth_evm::{BlockExecutionError, BlockValidationError, EvmError, InvalidTxError};

const DEPOSIT_BYTES_SIZE: usize = 48 + 32 + 8 + 96 + 8;
const BUILDER_DEPOSIT_REQUEST_TYPE: u8 = 0x03;
const BUILDER_EXIT_REQUEST_TYPE: u8 = 0x04;

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
    /// An attached EIP-7928 BAL did not cover a transaction state read.
    BlockAccessListNotCovered,
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
            Self::BlockAccessListNotCovered => {
                f.write_str("block access list does not cover transaction state reads")
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
            EthExecutionError::BlockAccessListNotCovered => {
                BlockValidationError::BlockAccessListNotCovered.into()
            }
            EthExecutionError::DepositRequestDecode(err) => {
                BlockValidationError::DepositRequestDecode(err).into()
            }
            EthExecutionError::SystemCallFailed { address, reason }
                if address == WITHDRAWAL_REQUEST_ADDRESS =>
            {
                BlockValidationError::WithdrawalRequestsContractCall { message: reason }.into()
            }
            EthExecutionError::SystemCallFailed { address, reason }
                if address == CONSOLIDATION_REQUEST_ADDRESS =>
            {
                BlockValidationError::ConsolidationRequestsContractCall { message: reason }.into()
            }
            err @ EthExecutionError::SystemCallFailed { .. } => {
                BlockValidationError::Other(Box::new(err)).into()
            }
            err => Self::other(err),
        }
    }
}

const fn handler_error_is_invalid_tx(err: &HandlerError) -> bool {
    !matches!(err, HandlerError::Fatal(_) | HandlerError::WrongTransactionType { .. })
}

/// Additional block-level execution context.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BlockExecutionContext<'a> {
    /// Pre-block system calls to run before transaction execution.
    pub system_calls: Option<BlockSystemCalls>,
    /// Pre-merge ommer headers included in the block.
    pub ommers: Option<&'a [Header]>,
    /// Post-block withdrawals to apply after transaction execution.
    pub withdrawals: Option<&'a [Withdrawal]>,
    /// Deposit contract address used to derive EIP-6110 deposit requests from receipts.
    pub deposit_contract_address: Option<Address>,
}

/// Inputs required by Ethereum pre-block system calls.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockSystemCalls {
    /// Parent block hash for EIP-2935 history storage.
    pub parent_hash: B256,
    /// Parent beacon block root for EIP-4788 beacon roots.
    pub parent_beacon_block_root: Option<B256>,
}

fn map_handler_error<T: EvmTypes>(evm: &mut Evm<'_, T>, err: HandlerError) -> EthExecutionError {
    match err {
        HandlerError::Fatal(code) => map_db_error_code(evm, code),
        err if handler_error_is_invalid_tx(&err) => {
            EthExecutionError::InvalidTx(EthInvalidTxError(err))
        }
        err => EthExecutionError::Handler(err),
    }
}

fn take_database_error<T: EvmTypes>(evm: &mut Evm<'_, T>, code: ErrorCode) -> DynamicDatabaseError {
    DynamicDatabaseError::new(evm.database_mut().error(code))
}

fn send_state_update(state: EvmState, on_state_update: &mut impl FnMut(EvmState)) {
    if !state.is_empty() {
        on_state_update(state);
    }
}

pub(crate) fn execute_transaction_with_condition<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    transaction: &Recovered<T::Tx>,
    commit: impl FnOnce(&TxResult<T>) -> reth_evm::CommitChanges,
) -> Result<Option<TxResult<T>>, EthExecutionError>
where
    T::Tx: Typed2718,
{
    let mut changes = TransactionChanges::default();
    let result = match evm.transact(transaction) {
        Ok(executed) => {
            if let Some(code) = executed.result().error_code {
                let _ = executed.discard();
                Err(HandlerError::Fatal(code))
            } else if commit(executed.result()).should_commit() {
                let Ok(result) = if stream_state {
                    executed.commit_with(&mut changes)
                } else {
                    executed.commit_with(&mut block_state.transaction_sink())
                };
                Ok(Some(result))
            } else {
                let _ = executed.discard();
                Ok(None)
            }
        }
        Err(error) => Err(error),
    };
    if stream_state {
        block_state.commit(&changes);
        send_state_update(changes.state, on_state_update);
    }
    result.map_err(|error| map_handler_error(evm, error))
}

pub(crate) fn execute_transaction_without_commit<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    transaction: &Recovered<T::Tx>,
) -> Result<TxResultWithState<T>, EthExecutionError>
where
    T::Tx: Typed2718,
{
    enum TransactionResolution<U: EvmTypes> {
        Outcome(TxResultWithState<U>),
        DatabaseError(ErrorCode),
        HandlerError(HandlerError),
    }

    let resolution = match evm.transact(transaction) {
        Ok(executed) => {
            if let Some(code) = executed.result().error_code {
                let _ = executed.discard();
                TransactionResolution::DatabaseError(code)
            } else {
                TransactionResolution::<T>::Outcome(executed.detach())
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

pub(crate) fn commit_detached_transaction<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    output: TxResultWithState<T>,
) -> TxResult<T> {
    let TxResultWithState { result, pending_state, .. } = output;
    accumulate_pending_state(block_state, stream_state, on_state_update, &pending_state);
    // Reattach the finalized transaction so evm2 retains its account capacity and recycles
    // storage maps for the next transaction instead of dropping the detached allocations.
    evm.state_mut().set_pending_state(pending_state);
    evm.state_mut().commit_transaction();
    result
}

pub(crate) fn commit_pending_state<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    pending_state: &evm2::evm::PendingState,
) {
    accumulate_pending_state(block_state, stream_state, on_state_update, pending_state);
    evm.overlay_db_mut().commit_pending(pending_state);
}

fn accumulate_pending_state(
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    pending_state: &evm2::evm::PendingState,
) {
    if stream_state {
        let mut changes = TransactionChanges::default();
        let Ok(()) = pending_state.visit(&mut changes);
        block_state.commit(&changes);
        send_state_update(changes.state, on_state_update);
    } else {
        let Ok(()) = pending_state.visit(&mut block_state.transaction_sink());
    }
}

fn map_db_error_code<T: EvmTypes>(evm: &mut Evm<'_, T>, code: ErrorCode) -> EthExecutionError {
    if code == ErrorCode::BAL_NOT_COVERED {
        EthExecutionError::BlockAccessListNotCovered
    } else {
        EthExecutionError::Database(take_database_error(evm, code))
    }
}

pub(crate) fn pre_execution_system_call_state_changes<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    spec_id: SpecId,
    block_number: u64,
    context: BlockExecutionContext<'_>,
) -> Result<(), EthExecutionError> {
    let Some(system_calls) = context.system_calls else {
        return Ok(());
    };

    if spec_id.enables(SpecId::PRAGUE) && block_number != 0 {
        let _ = execute_system_call(
            evm,
            block_state,
            stream_state,
            on_state_update,
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
                stream_state,
                on_state_update,
                BEACON_ROOTS_ADDRESS,
                parent_beacon_block_root.0.into(),
            )?;
        }
    }

    Ok(())
}

pub(crate) fn block_requests_from_receipts<R>(
    spec_id: SpecId,
    context: BlockExecutionContext<'_>,
    receipts: &[R],
) -> Result<Requests, EthExecutionError>
where
    R: TxReceipt<Log = Log>,
{
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

// TODO: Replace the local deposit event and encoding with alloy-eips helpers once
// https://github.com/alloy-rs/alloy/pull/4244 is released.
fn parse_deposit_requests_from_receipts<R>(
    deposit_contract_address: Address,
    receipts: &[R],
) -> Result<Vec<u8>, EthExecutionError>
where
    R: TxReceipt<Log = Log>,
{
    let mut out = Vec::new();
    for receipt in receipts {
        for log in receipt.logs() {
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

pub(crate) fn post_execution_system_call_state_changes<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    spec_id: SpecId,
    context: BlockExecutionContext<'_>,
    requests: &mut Requests,
) -> Result<(), EthExecutionError> {
    if context.system_calls.is_none() || !spec_id.enables(SpecId::PRAGUE) {
        return Ok(());
    }

    let withdrawal_requests = execute_system_call(
        evm,
        block_state,
        stream_state,
        on_state_update,
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
        stream_state,
        on_state_update,
        CONSOLIDATION_REQUEST_ADDRESS,
        Bytes::new(),
    )?;
    requests.push_request_with_type(
        CONSOLIDATION_REQUEST_TYPE,
        consolidation_requests.output.iter().copied(),
    );

    if spec_id.enables(SpecId::AMSTERDAM) {
        let builder_deposit_requests = execute_system_call(
            evm,
            block_state,
            stream_state,
            on_state_update,
            BUILDER_DEPOSIT_REQUEST_ADDRESS,
            Bytes::new(),
        )?;
        requests.push_request_with_type(
            BUILDER_DEPOSIT_REQUEST_TYPE,
            builder_deposit_requests.output.iter().copied(),
        );

        let builder_exit_requests = execute_system_call(
            evm,
            block_state,
            stream_state,
            on_state_update,
            BUILDER_EXIT_REQUEST_ADDRESS,
            Bytes::new(),
        )?;
        requests.push_request_with_type(
            BUILDER_EXIT_REQUEST_TYPE,
            builder_exit_requests.output.iter().copied(),
        );
    }

    Ok(())
}

fn execute_system_call<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    address: Address,
    data: Bytes,
) -> Result<TxResult<T>, EthExecutionError> {
    enum SystemCallResolution<U: EvmTypes> {
        Outcome(TxResultWithState<U>),
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
                let outcome = executed.detach();
                SystemCallResolution::<T>::Outcome(outcome)
            }
        }
        Err(err) => SystemCallResolution::HandlerError(err),
    };

    match resolution {
        SystemCallResolution::Outcome(outcome) => Ok(commit_detached_transaction(
            evm,
            block_state,
            stream_state,
            on_state_update,
            outcome,
        )),
        SystemCallResolution::DatabaseError(code) => Err(map_db_error_code(evm, code)),
        SystemCallResolution::HandlerError(err) => Err(map_handler_error(evm, err)),
        SystemCallResolution::Failed(reason) => {
            Err(EthExecutionError::SystemCallFailed { address, reason })
        }
    }
}

fn commit_state_changes<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    changes: &[(Address, Option<AccountInfo>, Option<AccountInfo>)],
) {
    let mut converted = TransactionChanges::default();
    for (address, original, current) in changes {
        let change = AccountChangeRef {
            address: *address,
            original: original.as_ref(),
            current: current.as_ref(),
            created: false,
            selfdestructed: false,
        };
        let Ok(()) = evm.overlay_db_mut().account(change);
        let Ok(()) = converted.account(change);
    }
    block_state.commit(&converted);
    if stream_state {
        send_state_update(converted.state, on_state_update);
    }
}

#[expect(clippy::too_many_arguments)]
pub(crate) fn post_block_balance_state_changes<T: EvmTypes>(
    evm: &mut Evm<'_, T>,
    block_state: &mut BlockState,
    stream_state: bool,
    on_state_update: &mut impl FnMut(EvmState),
    base_block_reward: Option<u128>,
    dao_fork_transition: bool,
    block_number: u64,
    block_beneficiary: Address,
    ommers: Option<&[Header]>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<(), EthExecutionError> {
    let mut balance_increments = AddressMap::<U256>::default();

    if let Some(base_block_reward) = base_block_reward {
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

    let mut changes = Vec::new();

    if dao_fork_transition {
        core::hint::cold_path();
        let mut drained_balance = U256::ZERO;
        for address in dao_fork::DAO_HARDFORK_ACCOUNTS {
            let original =
                evm.read_account_info(&address).map_err(|code| map_db_error_code(evm, code))?;
            let Some(original) = original else { continue };
            if original.balance.is_zero() {
                continue
            }

            drained_balance = drained_balance.saturating_add(original.balance);
            let mut current = original.clone();
            current.balance = U256::ZERO;
            changes.push((address, Some(original), Some(current)));
        }

        if !drained_balance.is_zero() {
            *balance_increments.entry(dao_fork::DAO_HARDFORK_BENEFICIARY).or_default() +=
                drained_balance;
        }
    }

    if balance_increments.is_empty() {
        return Ok(());
    }

    for (address, increment) in balance_increments {
        let original =
            evm.read_account_info(&address).map_err(|code| map_db_error_code(evm, code))?;
        let current = if increment.is_zero() && original.as_ref().is_none_or(AccountInfo::is_empty)
        {
            None
        } else {
            let mut current = original.clone().unwrap_or_else(AccountInfo::empty);
            current.balance = current.balance.saturating_add(increment);
            Some(current)
        };
        if original == current {
            // Zero-value withdrawals still count as account accesses in the BAL.
            let Ok(()) = evm.overlay_db_mut().account_read(address, original.as_ref());
            continue
        }
        changes.push((address, original, current));
    }

    commit_state_changes(evm, block_state, stream_state, on_state_update, &changes);

    Ok(())
}

pub(crate) fn base_block_reward<C>(chain_spec: &C, block_number: u64) -> Option<u128>
where
    C: EthereumHardforks + ?Sized,
{
    if chain_spec.is_paris_active_at_block(block_number) {
        None
    } else if chain_spec.is_constantinople_active_at_block(block_number) {
        Some(ETH_TO_WEI * 2)
    } else if chain_spec.is_byzantium_active_at_block(block_number) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxLegacy, TxType};
    use alloy_genesis::Genesis;
    use alloy_primitives::{address, keccak256, Signature, TxKind};
    use evm2::{
        bytecode::Bytecode, env::BlockEnv as EvmBlockEnv, evm::InMemoryDB, interpreter::opcode::op,
    };
    use reth_chainspec::{Chain, ChainSpec, MAINNET};
    use reth_ethereum_forks::{EthereumHardfork, ForkCondition};
    use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned};
    use reth_evm::{BlockExecutorFactory, ConfigureEvm, Executor};
    use reth_execution_types::BlockExecutionOutput;
    use reth_primitives_traits::RecoveredBlock;
    use reth_trie_common::{HashedPostState, KeccakKeyHasher};
    use std::sync::{mpsc, Arc};

    type BlockEnv = EvmBlockEnv<evm2::BaseEvmTypes>;

    #[test]
    fn detached_commits_preserve_storage_hooks_and_bal() {
        let caller = Address::with_last_byte(1);
        let contract = address!("0000000000000000000000000000000000001000");
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &caller,
            AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)),
        );
        database.insert_account_info(
            &contract,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(alloy_primitives::bytes!("5f546001015f5500"))),
        );
        let factory = crate::EthBlockExecutorFactory::new(MAINNET.clone());
        let mut expected = None;
        let mut expected_updates = None;
        for mode in 0..3 {
            for stream_state in [false, true] {
                let env = crate::EthEvmEnv::new(
                    SpecId::AMSTERDAM,
                    BlockEnv { gas_limit: U256::from(10_000_000), ..Default::default() },
                    1,
                );
                let mut evm = factory.evm_with_env(evm2::evm::Db::new(database.clone()), env);
                evm.state_mut().enable_bal_builder();
                let mut block = BlockState::new();
                let mut updates = Vec::new();
                for nonce in 0..2 {
                    evm.state_mut().set_bal_index(alloy_eip7928::BlockAccessIndex::new(nonce + 1));
                    let transaction = Recovered::new_unchecked(
                        evm2::ethereum::TxEnvelope::Legacy(TxLegacy {
                            nonce,
                            gas_limit: 1_000_000,
                            gas_price: 1,
                            to: TxKind::Call(contract),
                            ..Default::default()
                        }),
                        caller,
                    );
                    let mut hook = |state| updates.push(state);
                    if mode == 0 {
                        let result = execute_transaction_with_condition(
                            &mut evm,
                            &mut block,
                            stream_state,
                            &mut hook,
                            &transaction,
                            |_| reth_evm::CommitChanges::Yes,
                        )
                        .unwrap()
                        .unwrap();
                        assert!(result.status, "{result:?}");
                    } else {
                        let output =
                            execute_transaction_without_commit(&mut evm, &transaction).unwrap();
                        assert!(output.result.status, "{:?}", output.result);
                        if mode == 1 {
                            let _ = commit_detached_transaction(
                                &mut evm,
                                &mut block,
                                stream_state,
                                &mut hook,
                                output,
                            );
                        } else {
                            commit_pending_state(
                                &mut evm,
                                &mut block,
                                stream_state,
                                &mut hook,
                                &output.pending_state,
                            );
                        }
                    }
                }
                let bundle = block.into_bundle();
                assert_eq!(bundle.storage(&contract, U256::ZERO), Some(U256::from(2)));
                assert_eq!(updates.len(), if stream_state { 2 } else { 0 });
                if stream_state {
                    assert_eq!(expected_updates.get_or_insert_with(|| updates.clone()), &updates);
                }
                let output = (bundle, evm.state_mut().take_bal_builder());
                if let Some(expected) = &expected {
                    assert_eq!(&output, expected);
                } else {
                    expected = Some(output);
                }
            }
        }
    }

    fn execute_block(
        spec: SpecId,
        env: BlockEnv,
        database: InMemoryDB,
        transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
        withdrawals: Option<&[Withdrawal]>,
    ) -> Result<BlockExecutionOutput<Receipt>, BlockExecutionError> {
        let builder = ChainSpec::builder().chain(Chain::mainnet()).genesis(Genesis::default());
        let chain = match spec {
            SpecId::LONDON => builder.london_activated(),
            SpecId::SHANGHAI => builder.shanghai_activated(),
            SpecId::PRAGUE => builder.prague_activated(),
            SpecId::AMSTERDAM => builder.amsterdam_activated(),
            _ => panic!("unsupported test fork"),
        }
        .build();
        let (transactions, senders) = transactions.into_iter().map(Recovered::into_parts).unzip();
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header {
                    number: 1,
                    beneficiary: env.beneficiary,
                    gas_limit: env.gas_limit.to(),
                    timestamp: env.timestamp.to(),
                    base_fee_per_gas: Some(env.basefee.to()),
                    excess_blob_gas: Some(0),
                    parent_beacon_block_root: Some(B256::ZERO),
                    ..Default::default()
                },
                body: BlockBody {
                    transactions,
                    withdrawals: withdrawals.map(|withdrawals| withdrawals.to_vec().into()),
                    ..Default::default()
                },
            },
            senders,
        );
        let (tx, rx) = mpsc::channel();
        let config = crate::EthEvmConfig::new(Arc::new(chain));
        let without_hook = config.batch_executor(database.clone()).execute(&block)?;
        let output = config
            .batch_executor(database)
            .execute_with_state_hook(&block, move |state| tx.send(state).unwrap())?;
        assert_eq!(without_hook, output);
        let mut streamed = HashedPostState::default();
        for update in rx.try_iter() {
            for (address, account) in update {
                let hash = keccak256(address);
                if account.is_selfdestructed() || account.info != account.original_info() {
                    streamed.accounts.insert(
                        hash,
                        (!account.is_selfdestructed()).then(|| account.info.clone().into()),
                    );
                }
                if !account.is_selfdestructed() {
                    for (key, value) in account.storage {
                        if value.is_changed() {
                            streamed
                                .storages
                                .entry(hash)
                                .or_default()
                                .storage
                                .insert(keccak256(B256::from(key)), value.present_value);
                        }
                    }
                }
            }
        }
        let recomputed = HashedPostState::from_bundle_state::<KeccakKeyHasher>(&output.state.state);
        assert_eq!(streamed.into_sorted(), recomputed.into_sorted());
        Ok(output)
    }

    #[test]
    fn charges_london_sstore_set_gas() {
        let caller = address!("0000000000000000000000000000000000000001");
        let contract = address!("0000000000000000000000000000000000001000");
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &caller,
            AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)),
        );
        database.insert_account_info(
            &contract,
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
                    value: U256::from(1),
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
            [transaction],
            None,
        )
        .expect("EVM execution succeeds");

        assert_eq!(output.result.receipts[0].cumulative_gas_used, 43_106);
        assert!(output.result.receipts[0].success);
        assert_eq!(output.account(&contract).unwrap().unwrap().balance, U256::from(1));
        let sender = output.account(&caller).unwrap().unwrap();
        assert_eq!(sender.nonce, 1);
        assert_eq!(sender.balance, U256::from(ETH_TO_WEI) - U256::from(43_106_000_000_001u64));
        assert_eq!(output.storage(&contract, U256::ZERO).unwrap(), U256::from(10));
    }

    #[test]
    fn applies_withdrawals_to_block_output() {
        let existing = address!("0000000000000000000000000000000000000001");
        let new = address!("0000000000000000000000000000000000000002");
        let mut database = InMemoryDB::default();
        database
            .insert_account_info(&existing, AccountInfo::default().with_balance(U256::from(100)));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: existing, amount: 1 },
            Withdrawal { index: 1, validator_index: 1, address: new, amount: 2 },
            Withdrawal { index: 2, validator_index: 2, address: new, amount: 3 },
        ];

        let output = execute_block(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("EVM execution succeeds");

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.account_state(&existing).unwrap().info.as_ref().unwrap().balance,
            U256::from(1_000_000_100)
        );
        assert_eq!(
            output.account_state(&new).unwrap().info.as_ref().unwrap().balance,
            U256::from(5_000_000_000u64)
        );
    }

    #[test]
    fn zero_withdrawals_apply_eip161_state_clearing() {
        let nonexistent = address!("0000000000000000000000000000000000000001");
        let empty = address!("0000000000000000000000000000000000000002");
        let contract = address!("0000000000000000000000000000000000000003");
        let mut database = InMemoryDB::default();
        database.insert_account_info(&empty, AccountInfo::default());
        database.insert_account_info(&contract, AccountInfo::default().with_nonce(1));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: nonexistent, amount: 0 },
            Withdrawal { index: 1, validator_index: 1, address: empty, amount: 0 },
            Withdrawal { index: 2, validator_index: 2, address: contract, amount: 0 },
        ];

        let output = execute_block(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database.clone(),
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("EVM execution succeeds");

        assert!(output.account_state(&nonexistent).is_none());
        assert!(output.account_state(&empty).unwrap().info.is_none());
        assert!(output.account_state(&contract).is_none());
        let factory = crate::EthBlockExecutorFactory::new(MAINNET.clone());
        let env = crate::EthEvmEnv::new(SpecId::AMSTERDAM, BlockEnv::default(), 1);
        let mut evm = factory.evm_with_env(evm2::evm::Db::new(database), env);
        evm.state_mut().enable_bal_builder();
        post_block_balance_state_changes(
            &mut evm,
            &mut BlockState::new(),
            false,
            &mut |_| {},
            None,
            false,
            1,
            Address::ZERO,
            None,
            Some(&withdrawals),
        )
        .unwrap();
        let bal = evm.state_mut().take_bal_builder().unwrap();
        for withdrawal in withdrawals {
            assert!(bal.accounts.contains_key(&withdrawal.address));
        }
    }

    #[test]
    fn test_calc_base_block_reward() {
        // ((block number, td), reward)
        let cases = [
            // Pre-byzantium
            ((0, U256::ZERO), Some(ETH_TO_WEI * 5)),
            // Byzantium
            ((4370000, U256::ZERO), Some(ETH_TO_WEI * 3)),
            // Petersburg
            ((7280000, U256::ZERO), Some(ETH_TO_WEI * 2)),
            // Merge
            ((15537394, U256::from(58_750_000_000_000_000_000_000_u128)), None),
        ];

        for ((block_number, _td), expected_reward) in cases {
            assert_eq!(base_block_reward(&*MAINNET, block_number), expected_reward);
        }
    }

    #[test]
    fn test_calc_full_block_reward() {
        let base_reward = ETH_TO_WEI;
        let one_thirty_twoth_reward = base_reward >> 5;

        // (num_ommers, reward)
        let cases = [
            (0, base_reward),
            (1, base_reward + one_thirty_twoth_reward),
            (2, base_reward + one_thirty_twoth_reward * 2),
        ];

        for (num_ommers, expected_reward) in cases {
            assert_eq!(block_reward(base_reward, num_ommers), expected_reward);
        }
    }

    #[test]
    fn block_reward_uses_constantinople_activation() {
        let chain_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(EthereumHardfork::Byzantium, ForkCondition::Block(9))
            .with_fork(EthereumHardfork::Constantinople, ForkCondition::Block(12))
            .with_fork(EthereumHardfork::Petersburg, ForkCondition::Block(15))
            .with_fork(EthereumHardfork::Paris, ForkCondition::Never)
            .build();

        assert_eq!(base_block_reward(&chain_spec, 11), Some(ETH_TO_WEI * 3));
        let reward = base_block_reward(&chain_spec, 12).unwrap();
        assert_eq!(reward, ETH_TO_WEI * 2);
        assert_eq!(block_reward(reward, 1), ETH_TO_WEI * 2 + ((ETH_TO_WEI * 2) >> 5));
        assert_eq!(ommer_reward(reward, 12, 11), ETH_TO_WEI * 7 / 4);
        assert_eq!(base_block_reward(&chain_spec, 14), Some(ETH_TO_WEI * 2));
    }

    #[test]
    fn collects_post_execution_system_call_requests() {
        let mut database = InMemoryDB::default();
        database.insert_account_info(
            &WITHDRAWAL_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xaa)),
        );
        database.insert_account_info(
            &CONSOLIDATION_REQUEST_ADDRESS,
            AccountInfo::default().with_code(return_byte_code(0xbb)),
        );

        let output = execute_block(
            SpecId::PRAGUE,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            None,
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
    }

    #[test]
    fn collects_amsterdam_builder_requests() {
        let mut database = InMemoryDB::default();
        for (address, byte) in [
            (WITHDRAWAL_REQUEST_ADDRESS, 0xaa),
            (CONSOLIDATION_REQUEST_ADDRESS, 0xbb),
            (BUILDER_DEPOSIT_REQUEST_ADDRESS, 0xcc),
            (BUILDER_EXIT_REQUEST_ADDRESS, 0xdd),
        ] {
            database.insert_account_info(
                &address,
                AccountInfo::default().with_code(return_byte_code(byte)),
            );
        }

        let output = execute_block(
            SpecId::AMSTERDAM,
            BlockEnv::default(),
            database,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            None,
        )
        .expect("system calls succeed");

        assert_eq!(
            output.result.requests.iter().cloned().collect::<Vec<_>>(),
            vec![
                Bytes::from_static(&[WITHDRAWAL_REQUEST_TYPE, 0xaa]),
                Bytes::from_static(&[CONSOLIDATION_REQUEST_TYPE, 0xbb]),
                Bytes::from_static(&[BUILDER_DEPOSIT_REQUEST_TYPE, 0xcc]),
                Bytes::from_static(&[BUILDER_EXIT_REQUEST_TYPE, 0xdd]),
            ]
        );
    }

    #[test]
    fn failed_request_system_calls_are_validation_errors() {
        for address in [WITHDRAWAL_REQUEST_ADDRESS, CONSOLIDATION_REQUEST_ADDRESS] {
            for code in [
                Bytes::from_static(&[op::PUSH0, op::PUSH0, op::REVERT]),
                Bytes::from_static(&[op::INVALID]),
            ] {
                let mut database = InMemoryDB::default();
                database.insert_account_info(
                    &address,
                    AccountInfo::default().with_code(Bytecode::new_legacy(code)),
                );
                let error = execute_block(
                    SpecId::PRAGUE,
                    BlockEnv::default(),
                    database,
                    core::iter::empty::<Recovered<TransactionSigned>>(),
                    None,
                )
                .unwrap_err();
                assert!(error.as_validation().is_some(), "{error:?}");
            }
        }
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
