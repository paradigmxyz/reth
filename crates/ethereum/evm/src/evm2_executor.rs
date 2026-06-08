//! Evm2-backed Ethereum execution helpers.

use crate::{evm2_recovered_tx, RethReceiptBuilder};
use alloc::{format, string::String, vec::Vec};
use alloy_consensus::{constants::ETH_TO_WEI, transaction::Recovered, BlockHeader, Header};
use alloy_eips::{
    eip4895::Withdrawal, eip7002::WITHDRAWAL_REQUEST_TYPE, eip7251::CONSOLIDATION_REQUEST_TYPE,
    eip7685::Requests,
};
#[cfg(feature = "std")]
use alloy_primitives::{address, b256};
use alloy_primitives::{map::AddressMap, Address, Bytes, B256, KECCAK256_EMPTY, U256};
use evm2::{
    env::BlockEnv,
    ethereum::{ethereum_tx_registry, RecoveredTxEnvelope},
    evm::{
        AccountChangeRef, AccountInfo, BlockStateAccumulator, Database, Db, DbErrorCode,
        DynDatabase, StateChangeSink, StateChangeSource, StateChanges, StorageChangeRef, Tracked,
        BEACON_ROOTS_ADDRESS, CONSOLIDATION_REQUEST_ADDRESS, HISTORY_STORAGE_ADDRESS,
        WITHDRAWAL_REQUEST_ADDRESS,
    },
    registry::HandlerError,
    BaseEvmTypes, Evm, ExecutionConfig, Precompiles, SpecId, TxOutcome, Version,
};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_execution_types::BlockExecutionOutput;
#[cfg(feature = "std")]
use reth_storage_api::{
    BorrowedEvm2StateProviderDatabase, Evm2StateProviderDatabase, StateProvider,
};
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;
#[cfg(feature = "std")]
use tracing::warn;

/// Error returned by evm2-backed Ethereum execution.
#[derive(Debug)]
pub enum Evm2ExecutionError<E> {
    /// Evm2 rejected or halted transaction execution before producing a Reth output.
    Handler(HandlerError),
    /// Evm2 reported a database error and the typed database error was available.
    Database(E),
    /// Evm2 reported a database error, but the typed database error was no longer available.
    MissingDatabaseError(DbErrorCode),
    /// Cancun requires a parent beacon block root after genesis.
    MissingParentBeaconBlockRoot,
    /// Cancun genesis payloads must carry a zero parent beacon block root.
    CancunGenesisParentBeaconBlockRootNotZero(B256),
    /// A pre-block system call reverted or halted without producing a successful result.
    SystemCallFailed {
        /// System contract address that was called.
        address: Address,
        /// Evm2 stop reason for the failed call.
        reason: String,
    },
}

impl<E> core::fmt::Display for Evm2ExecutionError<E>
where
    E: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Handler(err) => write!(f, "evm2 execution error: {err}"),
            Self::Database(err) => write!(f, "evm2 database error: {err}"),
            Self::MissingDatabaseError(code) => {
                write!(f, "evm2 database error {code:?} was not available")
            }
            Self::MissingParentBeaconBlockRoot => {
                f.write_str("missing parent beacon block root for Cancun system call")
            }
            Self::CancunGenesisParentBeaconBlockRootNotZero(root) => {
                write!(f, "Cancun genesis parent beacon block root must be zero, got {root}")
            }
            Self::SystemCallFailed { address, reason } => {
                write!(f, "evm2 system call to {address} failed: {reason}")
            }
        }
    }
}

impl<E> core::error::Error for Evm2ExecutionError<E> where E: core::error::Error + Send + 'static {}

/// Additional block-level execution context for evm2.
#[derive(Debug, Clone, Copy)]
pub struct Evm2BlockExecutionContext<'a> {
    /// Chain id used for transaction validation and the `CHAINID` opcode.
    pub chain_id: u64,
    /// Pre-block system calls to run before transaction execution.
    pub system_calls: Option<Evm2BlockSystemCalls>,
    /// Pre-merge ommer headers included in the block.
    pub ommers: Option<&'a [Header]>,
    /// Post-block withdrawals to apply after transaction execution.
    pub withdrawals: Option<&'a [Withdrawal]>,
}

impl Default for Evm2BlockExecutionContext<'_> {
    fn default() -> Self {
        Self { chain_id: 1, system_calls: None, ommers: None, withdrawals: None }
    }
}

/// Inputs required by Ethereum pre-block system calls.
#[derive(Debug, Clone, Copy)]
pub struct Evm2BlockSystemCalls {
    /// Parent block hash for EIP-2935 history storage.
    pub parent_hash: B256,
    /// Parent beacon block root for EIP-4788 beacon roots.
    pub parent_beacon_block_root: Option<B256>,
}

/// Executes a block worth of recovered Ethereum transactions with evm2.
pub fn execute_evm2_block<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    execute_evm2_block_with_withdrawals(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        None,
    )
}

/// Executes a block worth of recovered Ethereum transactions and post-block withdrawals with evm2.
pub fn execute_evm2_block_with_withdrawals<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    execute_evm2_block_with_context(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        Evm2BlockExecutionContext { chain_id: 1, system_calls: None, ommers: None, withdrawals },
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context.
pub fn execute_evm2_block_with_context<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let block_beneficiary = block_env.beneficiary;
    let mut version = Version::new(spec_id);
    version.chain_id = context.chain_id;
    let mut evm = Evm::<BaseEvmTypes>::new_with_execution_config(
        ExecutionConfig::for_spec_and_version(spec_id, version),
        spec_id,
        block_env,
        ethereum_tx_registry(spec_id),
        Db::new(database),
        Precompiles::base(spec_id),
    );
    let mut block_state = BlockStateAccumulator::new();
    pre_execution_system_call_state_changes::<DB>(
        &mut evm,
        &mut block_state,
        spec_id,
        block_number,
        context,
    )?;
    let mut results = Vec::new();

    for (transaction_index, transaction) in transactions.into_iter().enumerate() {
        let tx_type = transaction.inner().tx_type();
        let transaction = evm2_recovered_tx(transaction);
        let outcome = execute_transaction::<DB>(
            &mut evm,
            &mut block_state,
            &transaction,
            block_number,
            transaction_index,
        )?;
        #[cfg(feature = "std")]
        if should_log_tx_diagnostics(block_number, transaction_index) {
            warn!(
                target: "reth::evm2::diagnostics",
                block_number,
                transaction_index,
                gas_used = outcome.gas_used,
                "evm2 transaction diagnostic"
            );
        }
        results.push((tx_type, outcome));
    }

    let requests = post_execution_system_call_state_changes::<DB>(
        &mut evm,
        &mut block_state,
        spec_id,
        context,
    )?;

    post_block_balance_state_changes::<DB>(
        &mut evm,
        &mut block_state,
        spec_id,
        block_number,
        block_beneficiary,
        context.ommers,
        context.withdrawals,
    )?;

    let mut output =
        RethReceiptBuilder.build_evm2_block_output_from_block_state(results, block_state.freeze());
    output.result.requests = requests;

    Ok(output)
}

/// Executes a block worth of recovered Ethereum transactions with an evm2 database adapter backed
/// by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
    )
}

/// Executes a block worth of recovered Ethereum transactions and withdrawals with an evm2 database
/// adapter backed by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider_and_withdrawals<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block_with_withdrawals(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
        withdrawals,
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context
/// and an evm2 database adapter backed by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider_context<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block_with_context(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
        context,
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context
/// and an evm2 database adapter backed by a borrowed Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_borrowed_state_provider_context(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: &dyn StateProvider,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>> {
    // SAFETY: The borrowed database is constructed and consumed by this synchronous execution call.
    // It is not stored beyond `execute_evm2_block_with_context`.
    let db = unsafe { BorrowedEvm2StateProviderDatabase::new(state_provider) };
    execute_evm2_block_with_context(spec_id, block_env, db, block_number, transactions, context)
}

fn map_handler_error<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    err: HandlerError,
) -> Evm2ExecutionError<DB::Error>
where
    DB: Database + 'static,
{
    match err {
        HandlerError::Database(code) => take_database_error::<DB>(evm)
            .map(Evm2ExecutionError::Database)
            .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code)),
        err => Evm2ExecutionError::Handler(err),
    }
}

fn take_database_error<DB>(evm: &mut Evm<BaseEvmTypes>) -> Option<DB::Error>
where
    DB: Database + 'static,
{
    evm.database_as_mut::<Db<DB>>().and_then(Db::take_result)
}

fn execute_transaction<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    transaction: &RecoveredTxEnvelope,
    block_number: u64,
    transaction_index: usize,
) -> Result<TxOutcome, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    enum TransactionResolution {
        Outcome(TxOutcome),
        DatabaseError(DbErrorCode),
        HandlerError(HandlerError),
    }

    #[cfg(feature = "std")]
    if should_log_tx_diagnostics(block_number, transaction_index) {
        log_pre_tx_diagnostics::<DB>(evm, block_number, transaction_index)?;
    }

    let resolution = match evm.transact(transaction) {
        Ok(executed) => {
            if let Some(code) = executed.outcome().db_error_code {
                executed.discard();
                TransactionResolution::DatabaseError(code)
            } else if should_log_tx_diagnostics(block_number, transaction_index) {
                #[cfg(feature = "std")]
                {
                    let mut sink = DiagnosticStateChangeSink {
                        inner: block_state,
                        block_number,
                        transaction_index,
                    };
                    match executed.commit_with(&mut sink) {
                        Ok(outcome) => TransactionResolution::Outcome(outcome),
                        Err(err) => match err {},
                    }
                }
                #[cfg(not(feature = "std"))]
                {
                    TransactionResolution::Outcome(executed.commit_to(block_state))
                }
            } else {
                TransactionResolution::Outcome(executed.commit_to(block_state))
            }
        }
        Err(err) => TransactionResolution::HandlerError(err),
    };

    match resolution {
        TransactionResolution::Outcome(outcome) => Ok(outcome),
        TransactionResolution::DatabaseError(code) => Err(map_db_error_code::<DB>(evm, code)),
        TransactionResolution::HandlerError(err) => Err(map_handler_error::<DB>(evm, err)),
    }
}

fn should_log_tx_diagnostics(block_number: u64, transaction_index: usize) -> bool {
    block_number == 25_266_573 && transaction_index == 131
}

#[cfg(feature = "std")]
fn log_pre_tx_diagnostics<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_number: u64,
    transaction_index: usize,
) -> Result<(), Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    for address in [
        address!("7ddd68906c0b3c0f7b507b942b2d0c3de00fd07b"),
        address!("9e6b1022be9bbf5afd152483dad9b88911bc8611"),
        address!("48e6c30b97748d1e2e03bf3e9fbe3890ca5f8cca"),
        address!("f411903cbc70a74d22900a5de66a2dda66507255"),
        address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
    ] {
        let account =
            evm.account_info(&address).map_err(|code| map_db_error_code::<DB>(evm, code))?;
        warn!(
            target: "reth::evm2::diagnostics",
            block_number,
            transaction_index,
            ?address,
            nonce = account.as_ref().map(|account| account.nonce),
            balance = ?account.as_ref().map(|account| account.balance),
            code_hash = ?account.as_ref().map(|account| account.code_hash),
            "evm2 pre tx account"
        );
    }

    for (address, key) in [
        (
            address!("48e6c30b97748d1e2e03bf3e9fbe3890ca5f8cca"),
            diagnostic_word(b256!(
                "0000000000000000000000000000000000000000000000000000000000000067"
            )),
        ),
        (
            address!("48e6c30b97748d1e2e03bf3e9fbe3890ca5f8cca"),
            diagnostic_word(b256!(
                "0000000000000000000000000000000000000000000000000000000000000087"
            )),
        ),
        (
            address!("48e6c30b97748d1e2e03bf3e9fbe3890ca5f8cca"),
            diagnostic_word(b256!(
                "0000000000000000000000000000000000000000000000000000000000000068"
            )),
        ),
        (
            address!("f411903cbc70a74d22900a5de66a2dda66507255"),
            diagnostic_word(b256!(
                "3c2dd7f9bf16609d7a472ad818cb2793c8a044504f64347eb9e62914cfa65ec8"
            )),
        ),
        (
            address!("f411903cbc70a74d22900a5de66a2dda66507255"),
            diagnostic_word(b256!(
                "257b92bcf49981a3fdf6bd5c205a5fd789ea9aa6e0eb98ce896df279ee93aca6"
            )),
        ),
        (
            address!("f411903cbc70a74d22900a5de66a2dda66507255"),
            diagnostic_word(b256!(
                "df7de25b7f1fd6d0b5205f0e18f1f35bd7b8d84cce336588d184533ce43a6f76"
            )),
        ),
        (
            address!("f411903cbc70a74d22900a5de66a2dda66507255"),
            diagnostic_word(b256!(
                "c9d7e5361495aac4cf1b8cbd68421981b8241fc3688cf509a24ca9483ab0e744"
            )),
        ),
        (
            address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
            diagnostic_word(b256!(
                "9c9ac1e9c0173ba157c0c3148144deca1941988af9242d9758c9b924acb7fadc"
            )),
        ),
    ] {
        let value = evm
            .overlay_db_mut()
            .get_storage(&address, &key)
            .map_err(|code| map_db_error_code::<DB>(evm, code))?;
        warn!(
            target: "reth::evm2::diagnostics",
            block_number,
            transaction_index,
            ?address,
            ?key,
            ?value,
            "evm2 pre tx storage"
        );
    }

    Ok(())
}

#[cfg(feature = "std")]
fn diagnostic_word(value: B256) -> U256 {
    U256::from_be_bytes(value.0)
}

#[cfg(feature = "std")]
struct DiagnosticStateChangeSink<'a> {
    inner: &'a mut BlockStateAccumulator,
    block_number: u64,
    transaction_index: usize,
}

#[cfg(feature = "std")]
impl StateChangeSink for DiagnosticStateChangeSink<'_> {
    type Error = core::convert::Infallible;

    fn bytecode(
        &mut self,
        code_hash: B256,
        code: &evm2::bytecode::Bytecode,
    ) -> Result<(), Self::Error> {
        warn!(
            target: "reth::evm2::diagnostics",
            block_number = self.block_number,
            transaction_index = self.transaction_index,
            ?code_hash,
            code_len = code.original_bytes().len(),
            "evm2 tx state bytecode"
        );
        self.inner.bytecode(code_hash, code)
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        warn!(
            target: "reth::evm2::diagnostics",
            block_number = self.block_number,
            transaction_index = self.transaction_index,
            address = ?change.address,
            original_nonce = change.original.as_ref().map(|account| account.nonce),
            current_nonce = change.current.as_ref().map(|account| account.nonce),
            original_balance = ?change.original.as_ref().map(|account| account.balance),
            current_balance = ?change.current.as_ref().map(|account| account.balance),
            original_code_hash = ?change.original.as_ref().map(|account| account.code_hash),
            current_code_hash = ?change.current.as_ref().map(|account| account.code_hash),
            "evm2 tx state account"
        );
        self.inner.account(change)
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        warn!(
            target: "reth::evm2::diagnostics",
            block_number = self.block_number,
            transaction_index = self.transaction_index,
            ?address,
            "evm2 tx state storage wipe"
        );
        self.inner.storage_wipe(address)
    }

    fn storage(&mut self, change: StorageChangeRef) -> Result<(), Self::Error> {
        warn!(
            target: "reth::evm2::diagnostics",
            block_number = self.block_number,
            transaction_index = self.transaction_index,
            address = ?change.address,
            key = ?change.key,
            original = ?change.original,
            current = ?change.current,
            "evm2 tx state storage"
        );
        self.inner.storage(change)
    }
}

fn map_db_error_code<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    code: DbErrorCode,
) -> Evm2ExecutionError<DB::Error>
where
    DB: Database + 'static,
{
    take_database_error::<DB>(evm)
        .map(Evm2ExecutionError::Database)
        .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code))
}

fn pre_execution_system_call_state_changes<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    spec_id: SpecId,
    block_number: u64,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<(), Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let Some(system_calls) = context.system_calls else {
        return Ok(());
    };

    if spec_id.enables(SpecId::PRAGUE) && block_number != 0 {
        execute_system_call::<DB>(
            evm,
            block_state,
            HISTORY_STORAGE_ADDRESS,
            system_calls.parent_hash.0.into(),
        )?;
    }

    if spec_id.enables(SpecId::CANCUN) {
        let parent_beacon_block_root = system_calls
            .parent_beacon_block_root
            .ok_or(Evm2ExecutionError::MissingParentBeaconBlockRoot)?;

        if block_number == 0 {
            if parent_beacon_block_root != B256::ZERO {
                return Err(Evm2ExecutionError::CancunGenesisParentBeaconBlockRootNotZero(
                    parent_beacon_block_root,
                ));
            }
        } else {
            execute_system_call::<DB>(
                evm,
                block_state,
                BEACON_ROOTS_ADDRESS,
                parent_beacon_block_root.0.into(),
            )?;
        }
    }

    Ok(())
}

fn post_execution_system_call_state_changes<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    spec_id: SpecId,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<Requests, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let mut requests = Requests::default();
    if context.system_calls.is_none() || !spec_id.enables(SpecId::PRAGUE) {
        return Ok(requests);
    }

    let withdrawal_requests =
        execute_system_call::<DB>(evm, block_state, WITHDRAWAL_REQUEST_ADDRESS, Bytes::new())?;
    requests.push_request_with_type(
        WITHDRAWAL_REQUEST_TYPE,
        withdrawal_requests.output.iter().copied(),
    );

    let consolidation_requests =
        execute_system_call::<DB>(evm, block_state, CONSOLIDATION_REQUEST_ADDRESS, Bytes::new())?;
    requests.push_request_with_type(
        CONSOLIDATION_REQUEST_TYPE,
        consolidation_requests.output.iter().copied(),
    );

    Ok(requests)
}

fn execute_system_call<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    address: Address,
    data: Bytes,
) -> Result<TxOutcome, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let executed = evm.system_call(address, data);

    if let Some(code) = executed.outcome().db_error_code {
        executed.discard();
        return Err(map_db_error_code::<DB>(evm, code))
    }

    if !executed.outcome().status {
        let reason = format!("{:?}", executed.outcome().stop);
        executed.discard();
        return Err(Evm2ExecutionError::SystemCallFailed { address, reason });
    }

    Ok(executed.commit_to(block_state))
}

fn commit_state_changes<S: StateChangeSource>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    changes: &S,
) {
    evm.commit_source(changes);
    match changes.visit(block_state) {
        Ok(()) => {}
        Err(err) => match err {},
    }
}

fn post_block_balance_state_changes<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_state: &mut BlockStateAccumulator,
    spec_id: SpecId,
    block_number: u64,
    block_beneficiary: Address,
    ommers: Option<&[Header]>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<(), Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
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
            evm.account_info(&address).map_err(|code| map_db_error_code::<DB>(evm, code))?;
        let mut current = original.clone().unwrap_or_else(empty_account);

        current.balance = current.balance.saturating_add(increment);
        changes
            .accounts
            .insert(address, Tracked { original, current: Some(current), _non_exhaustive: () });
    }

    commit_state_changes(evm, block_state, &changes);

    Ok(())
}

fn base_block_reward(spec_id: SpecId) -> Option<u128> {
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

fn empty_account() -> AccountInfo {
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
    use alloy_primitives::{address, Address, Bytes, Signature, TxKind, B256, U256};
    use core::convert::Infallible;
    use evm2::{
        bytecode::Bytecode,
        evm::AccountInfo,
        interpreter::{opcode::op, Word},
    };

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

    #[test]
    fn executes_legacy_transfer_with_evm2() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1,
                    gas_limit: 21_000,
                    to: TxKind::Call(target),
                    value: U256::from(1),
                    input: Bytes::new(),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let output =
            execute_evm2_block(SpecId::FRONTIER, BlockEnv::default(), database, 1, [transaction])
                .expect("evm2 execution succeeds");

        assert_eq!(output.result.gas_used, 21_000);
        assert_eq!(output.result.receipts.len(), 1);
        assert!(output.result.receipts[0].success);
        assert_eq!(
            output.account_state(&target).unwrap().current.as_ref().unwrap().balance,
            U256::from(1)
        );
    }

    #[test]
    fn charges_london_sstore_set_gas_with_evm2() {
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

        let output = execute_evm2_block(
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
        .expect("evm2 execution succeeds");

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
        let err = execute_evm2_block(SpecId::FRONTIER, block_env, database, 1, [transaction])
            .expect_err("transaction gas limit above block gas limit should fail");

        assert!(matches!(
            err,
            Evm2ExecutionError::Handler(HandlerError::GasLimitMoreThanBlock {
                gas_limit,
                block_gas_limit,
            }) if gas_limit == 2_500_000 &&
                block_gas_limit == U256::from(1_500_000)
        ));
    }

    #[test]
    fn applies_withdrawals_to_evm2_block_output() {
        let existing = address!("0000000000000000000000000000000000000001");
        let new = address!("0000000000000000000000000000000000000002");
        let mut database = TestDatabase::default();
        database.accounts.insert(existing, AccountInfo::default().with_balance(U256::from(100)));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: existing, amount: 1 },
            Withdrawal { index: 1, validator_index: 1, address: new, amount: 2 },
            Withdrawal { index: 2, validator_index: 2, address: new, amount: 3 },
        ];

        let output = execute_evm2_block_with_withdrawals(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("evm2 execution succeeds");

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.account_state(&existing).unwrap().current.as_ref().unwrap().balance,
            U256::from(1_000_000_100)
        );
        assert_eq!(
            output.account_state(&new).unwrap().current.as_ref().unwrap().balance,
            U256::from(5_000_000_000u64)
        );
    }

    #[test]
    fn rejects_missing_cancun_parent_beacon_root() {
        let err = execute_evm2_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: None,
                }),
                ommers: None,
                withdrawals: None,
            },
        )
        .expect_err("missing parent beacon block root should fail");

        assert!(matches!(err, Evm2ExecutionError::MissingParentBeaconBlockRoot));
    }

    #[test]
    fn rejects_nonzero_cancun_genesis_parent_beacon_root() {
        let root = B256::from([1u8; 32]);
        let err = execute_evm2_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            0,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(root),
                }),
                ommers: None,
                withdrawals: None,
            },
        )
        .expect_err("nonzero Cancun genesis parent beacon block root should fail");

        assert!(matches!(
            err,
            Evm2ExecutionError::CancunGenesisParentBeaconBlockRootNotZero(actual) if actual == root
        ));
    }

    #[test]
    fn writes_beacon_root_contract_storage_with_evm2() {
        let mut database = TestDatabase::default();
        database.accounts.insert(
            BEACON_ROOTS_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(BEACON_ROOTS_CODE.clone())),
        );

        let timestamp = U256::from(1);
        let parent_beacon_block_root = B256::with_last_byte(0x69);
        let output = execute_evm2_block_with_context(
            SpecId::CANCUN,
            BlockEnv { number: U256::from(1), timestamp, ..Default::default() },
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(parent_beacon_block_root),
                }),
                ommers: None,
                withdrawals: None,
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
    }

    #[test]
    fn writes_parent_hash_history_storage_with_evm2() {
        let mut database = TestDatabase::default();
        database.accounts.insert(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
        );

        let parent_hash = B256::with_last_byte(0x42);
        let output = execute_evm2_block_with_context(
            SpecId::PRAGUE,
            BlockEnv { number: U256::from(1), ..Default::default() },
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
            },
        )
        .expect("history storage system call succeeds");

        assert_eq!(
            output.storage(&HISTORY_STORAGE_ADDRESS, U256::ZERO).unwrap(),
            U256::from_be_bytes(parent_hash.0)
        );
    }

    #[test]
    fn runs_pre_execution_system_calls_without_receipts() {
        let output = execute_evm2_block_with_context(
            SpecId::PRAGUE,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::from([2u8; 32]),
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
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

        let output = execute_evm2_block_with_context(
            SpecId::PRAGUE,
            BlockEnv::default(),
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
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
    }

    #[test]
    fn executes_withdrawal_request_contract_with_evm2() {
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

        let output = execute_evm2_block_with_context(
            SpecId::PRAGUE,
            BlockEnv { gas_limit: U256::from(1_500_000), ..Default::default() },
            database,
            1,
            [transaction],
            Evm2BlockExecutionContext {
                chain_id: 1,
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                ommers: None,
                withdrawals: None,
            },
        )
        .expect("withdrawal request transaction succeeds");

        assert!(output.result.receipts.first().unwrap().success);
        assert_eq!(output.result.requests.len(), 1);
        assert_eq!(output.result.requests[0][0], WITHDRAWAL_REQUEST_TYPE);
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
