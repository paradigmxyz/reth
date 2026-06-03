//! Helpers for integrating `evm2` execution output with reth execution types.

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use alloy_consensus::{
    transaction::Recovered, EthereumReceipt, EthereumTxEnvelope, TxEip4844, TxType,
};
use alloy_eip7928::BlockAccessList;
use alloy_eips::{
    eip4844::DATA_GAS_PER_BLOB,
    eip4895::Withdrawal,
    eip6110::{DEPOSIT_REQUEST_TYPE, MAINNET_DEPOSIT_CONTRACT_ADDRESS},
    eip7002::WITHDRAWAL_REQUEST_TYPE,
    eip7251::CONSOLIDATION_REQUEST_TYPE,
    eip7685::Requests,
};
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult as AlloyBlockExecutionResult,
        BlockExecutor as AlloyBlockExecutor, BlockExecutorFactory as AlloyBlockExecutorFactory,
        BlockValidationError, CommitChanges, ExecutableTx, GasOutput, StateDB,
        TxResult as AlloyTxResult,
    },
    eth::{dao_fork, EthBlockExecutionCtx},
    precompiles::PrecompilesMap,
    Evm as AlloyEvm, EvmFactory as AlloyEvmFactory, FromRecoveredTx, FromTxWithEncoded,
    RecoveredTx,
};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    Address, Bytes, B256, U256,
};
use core::{error::Error, fmt, ptr::NonNull};
use evm2::{
    bytecode::{Bytecode as Evm2Bytecode, JumpTable as Evm2JumpTable},
    env::BlockEnv as Evm2BlockEnv,
    ethereum::{ethereum_tx_registry, RecoveredTxEnvelope},
    evm::{
        AccountInfo as Evm2AccountInfo, Database as Evm2DatabaseTrait, Db as Evm2Db,
        DbErrorCode as Evm2DbErrorCode, Evm as Evm2, StateChanges, StorageChangeSet,
        TxResult as Evm2TxResult, BEACON_ROOTS_ADDRESS, CONSOLIDATION_REQUEST_ADDRESS,
        HISTORY_STORAGE_ADDRESS, WITHDRAWAL_REQUEST_ADDRESS,
    },
    interpreter::{Host, InstrStop},
    precompiles::Precompiles,
    SpecId,
};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult};
use revm::{
    context::{
        result::{ExecutionResult, HaltReason, Output, ResultAndState, ResultGas, SuccessReason},
        BlockEnv,
    },
    database::states::{
        reverts::{AccountInfoRevert, Reverts},
        AccountRevert, AccountStatus, BundleAccount, BundleState, RevertToSlot, StorageSlot,
        StorageWithOriginalValues,
    },
    inspector::Inspector,
    primitives::{hardfork::SpecId as RevmSpecId, StorageKeyMap},
    state::{
        Account as RevmAccount, AccountInfo, Bytecode, EvmState, EvmStorageSlot, TransactionId,
    },
    DatabaseCommit,
};
#[cfg(feature = "std")]
use std::sync::OnceLock;

const PRECOMPILE_CACHE_SIZE: usize = 7;
const ONE_ETHER: u128 = 1_000_000_000_000_000_000;

const fn precompile_cache_index(spec: SpecId) -> usize {
    match spec {
        SpecId::FRONTIER | SpecId::HOMESTEAD | SpecId::TANGERINE | SpecId::SPURIOUS_DRAGON => 0,
        SpecId::BYZANTIUM | SpecId::PETERSBURG => 1,
        SpecId::ISTANBUL => 2,
        SpecId::BERLIN | SpecId::LONDON | SpecId::MERGE | SpecId::SHANGHAI => 3,
        SpecId::CANCUN => 4,
        SpecId::PRAGUE => 5,
        _ => 6,
    }
}

/// Returns the cached evm2 precompile provider for the given spec.
#[cfg(feature = "std")]
pub fn precompile_cache_for_spec(spec: SpecId) -> &'static Precompiles {
    static CACHE: [OnceLock<Precompiles>; PRECOMPILE_CACHE_SIZE] =
        [const { OnceLock::new() }; PRECOMPILE_CACHE_SIZE];

    CACHE[precompile_cache_index(spec)].get_or_init(|| Precompiles::base(spec))
}

/// Returns an evm2 precompile provider for the given spec.
#[cfg(feature = "std")]
pub fn precompiles_for_spec(spec: SpecId) -> Precompiles {
    precompile_cache_for_spec(spec).clone()
}

/// Returns an evm2 precompile provider for the given spec.
#[cfg(not(feature = "std"))]
pub fn precompiles_for_spec(spec: SpecId) -> Precompiles {
    Precompiles::base(spec)
}

/// Concrete Ethereum evm2 host type used by reth integration code.
pub type EthEvm2 = Evm2<evm2::BaseEvmTypes>;

/// Ethereum transaction envelope used by reth's Ethereum primitives.
pub type RethEthereumTxEnvelope = EthereumTxEnvelope<TxEip4844>;

/// Database adapter that lets a reth/revm database back a concrete evm2 host.
#[derive(Debug)]
pub struct Evm2Database<DB> {
    inner: DB,
}

impl<DB> Evm2Database<DB> {
    /// Creates a new evm2 database adapter.
    pub const fn new(inner: DB) -> Self {
        Self { inner }
    }

    /// Returns the wrapped database.
    pub const fn inner(&self) -> &DB {
        &self.inner
    }

    /// Returns the wrapped database mutably.
    pub const fn inner_mut(&mut self) -> &mut DB {
        &mut self.inner
    }

    /// Consumes the adapter and returns the wrapped database.
    pub fn into_inner(self) -> DB {
        self.inner
    }
}

impl<DB> Evm2DatabaseTrait for Evm2Database<DB>
where
    DB: crate::Database + Send + 'static,
{
    type Error = DB::Error;

    fn get_account(
        &mut self,
        address: &alloy_primitives::Address,
    ) -> Result<Option<Evm2AccountInfo>, Self::Error> {
        self.inner.basic(*address).map(|info| info.map(account_info_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Evm2Bytecode, Self::Error> {
        self.inner.code_by_hash(*code_hash).map(bytecode_to_evm2)
    }

    fn get_storage(
        &mut self,
        address: &alloy_primitives::Address,
        key: &U256,
    ) -> Result<U256, Self::Error> {
        self.inner.storage(*address, *key)
    }

    fn get_block_hash(&mut self, number: &U256) -> Result<Option<B256>, Self::Error> {
        Ok(Some(self.inner.block_hash(number.saturating_to())?))
    }
}

type Evm2DatabaseRefResult<T> = Result<T, Evm2DatabaseRefError>;

/// Error returned by the borrowed evm2 database adapter.
#[derive(Debug)]
pub struct Evm2DatabaseRefError(Box<dyn Error + Send + Sync>);

impl fmt::Display for Evm2DatabaseRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for Evm2DatabaseRefError {}

/// Borrowed database adapter used when evm2 is driven through reth's block executor traits.
pub struct Evm2DatabaseRef {
    ptr: NonNull<()>,
    get_account: unsafe fn(NonNull<()>, &Address) -> Evm2DatabaseRefResult<Option<Evm2AccountInfo>>,
    get_code_by_hash: unsafe fn(NonNull<()>, &B256) -> Evm2DatabaseRefResult<Evm2Bytecode>,
    get_storage: unsafe fn(NonNull<()>, &Address, &U256) -> Evm2DatabaseRefResult<U256>,
    get_block_hash: unsafe fn(NonNull<()>, &U256) -> Evm2DatabaseRefResult<Option<B256>>,
}

impl fmt::Debug for Evm2DatabaseRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Evm2DatabaseRef").finish_non_exhaustive()
    }
}

unsafe impl Send for Evm2DatabaseRef {}

impl Evm2DatabaseRef {
    /// Creates a borrowed evm2 database adapter.
    pub fn new<DB>(db: &mut DB) -> Self
    where
        DB: crate::Database,
    {
        Self {
            ptr: NonNull::from(db).cast(),
            get_account: evm2_database_ref_get_account::<DB>,
            get_code_by_hash: evm2_database_ref_get_code_by_hash::<DB>,
            get_storage: evm2_database_ref_get_storage::<DB>,
            get_block_hash: evm2_database_ref_get_block_hash::<DB>,
        }
    }
}

impl Evm2DatabaseTrait for Evm2DatabaseRef {
    type Error = Evm2DatabaseRefError;

    fn get_account(&mut self, address: &Address) -> Result<Option<Evm2AccountInfo>, Self::Error> {
        // SAFETY: `ptr` is created from the live EVM database in `Evm2DatabaseRef::new`, and the
        // evm2 host is dropped before the EVM that owns that database.
        unsafe { (self.get_account)(self.ptr, address) }
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Evm2Bytecode, Self::Error> {
        // SAFETY: See `get_account`.
        unsafe { (self.get_code_by_hash)(self.ptr, code_hash) }
    }

    fn get_storage(&mut self, address: &Address, key: &U256) -> Result<U256, Self::Error> {
        // SAFETY: See `get_account`.
        unsafe { (self.get_storage)(self.ptr, address, key) }
    }

    fn get_block_hash(&mut self, number: &U256) -> Result<Option<B256>, Self::Error> {
        // SAFETY: See `get_account`.
        unsafe { (self.get_block_hash)(self.ptr, number) }
    }
}

unsafe fn evm2_database_ref_get_account<DB>(
    ptr: NonNull<()>,
    address: &Address,
) -> Evm2DatabaseRefResult<Option<Evm2AccountInfo>>
where
    DB: crate::Database,
{
    // SAFETY: The vtable functions are installed for the same `DB` used to create the pointer.
    let db = unsafe { &mut *ptr.cast::<DB>().as_ptr() };
    db.basic(*address)
        .map(|info| info.map(account_info_to_evm2))
        .map_err(|err| Evm2DatabaseRefError(Box::new(err)))
}

unsafe fn evm2_database_ref_get_code_by_hash<DB>(
    ptr: NonNull<()>,
    code_hash: &B256,
) -> Evm2DatabaseRefResult<Evm2Bytecode>
where
    DB: crate::Database,
{
    // SAFETY: The vtable functions are installed for the same `DB` used to create the pointer.
    let db = unsafe { &mut *ptr.cast::<DB>().as_ptr() };
    db.code_by_hash(*code_hash)
        .map(bytecode_to_evm2)
        .map_err(|err| Evm2DatabaseRefError(Box::new(err)))
}

unsafe fn evm2_database_ref_get_storage<DB>(
    ptr: NonNull<()>,
    address: &Address,
    key: &U256,
) -> Evm2DatabaseRefResult<U256>
where
    DB: crate::Database,
{
    // SAFETY: The vtable functions are installed for the same `DB` used to create the pointer.
    let db = unsafe { &mut *ptr.cast::<DB>().as_ptr() };
    db.storage(*address, *key).map_err(|err| Evm2DatabaseRefError(Box::new(err)))
}

unsafe fn evm2_database_ref_get_block_hash<DB>(
    ptr: NonNull<()>,
    number: &U256,
) -> Evm2DatabaseRefResult<Option<B256>>
where
    DB: crate::Database,
{
    // SAFETY: The vtable functions are installed for the same `DB` used to create the pointer.
    let db = unsafe { &mut *ptr.cast::<DB>().as_ptr() };
    db.block_hash(number.saturating_to())
        .map(Some)
        .map_err(|err| Evm2DatabaseRefError(Box::new(err)))
}

/// Converts a revm spec ID into evm2's spec ID.
pub const fn spec_id_from_revm(spec: RevmSpecId) -> SpecId {
    match spec {
        RevmSpecId::FRONTIER => SpecId::FRONTIER,
        RevmSpecId::HOMESTEAD => SpecId::HOMESTEAD,
        RevmSpecId::TANGERINE => SpecId::TANGERINE,
        RevmSpecId::SPURIOUS_DRAGON => SpecId::SPURIOUS_DRAGON,
        RevmSpecId::BYZANTIUM => SpecId::BYZANTIUM,
        RevmSpecId::PETERSBURG => SpecId::PETERSBURG,
        RevmSpecId::ISTANBUL => SpecId::ISTANBUL,
        RevmSpecId::BERLIN => SpecId::BERLIN,
        RevmSpecId::LONDON => SpecId::LONDON,
        RevmSpecId::MERGE => SpecId::MERGE,
        RevmSpecId::SHANGHAI => SpecId::SHANGHAI,
        RevmSpecId::CANCUN => SpecId::CANCUN,
        RevmSpecId::PRAGUE => SpecId::PRAGUE,
        RevmSpecId::OSAKA => SpecId::OSAKA,
        RevmSpecId::AMSTERDAM => SpecId::AMSTERDAM,
    }
}

/// Converts a revm block environment into evm2's block environment.
pub fn block_env_from_revm(block: BlockEnv) -> Evm2BlockEnv {
    Evm2BlockEnv {
        number: block.number,
        beneficiary: block.beneficiary,
        timestamp: block.timestamp,
        gas_limit: U256::from(block.gas_limit),
        basefee: U256::from(block.basefee),
        difficulty: block.difficulty,
        prevrandao: block.prevrandao.unwrap_or_default().into(),
        blob_basefee: block
            .blob_excess_gas_and_price
            .map(|blob| U256::from(blob.blob_gasprice))
            .unwrap_or_default(),
        slot_num: U256::from(block.slot_num),
        ext: (),
        _non_exhaustive: (),
    }
}

/// Creates a concrete Ethereum evm2 host backed by the given database.
pub fn create_evm2<DB>(db: DB, spec: SpecId, block: Evm2BlockEnv) -> EthEvm2
where
    DB: crate::Database + Send + 'static,
{
    Evm2::new(
        spec,
        block,
        ethereum_tx_registry(spec),
        Evm2Db::new(Evm2Database::new(db)),
        precompiles_for_spec(spec),
    )
}

/// Creates a concrete Ethereum evm2 host from a revm spec and block environment.
pub fn create_evm2_from_revm_env<DB>(db: DB, spec: RevmSpecId, block: BlockEnv) -> EthEvm2
where
    DB: crate::Database + Send + 'static,
{
    create_evm2(db, spec_id_from_revm(spec), block_env_from_revm(block))
}

fn create_evm2_from_alloy_evm<E>(evm: &mut E) -> EthEvm2
where
    E: AlloyEvm<Spec = RevmSpecId, BlockEnv = BlockEnv>,
    E::DB: crate::Database,
{
    let spec = spec_id_from_revm(*evm.cfg_env().spec());
    Evm2::new(
        spec,
        block_env_from_revm(evm.block().clone()),
        ethereum_tx_registry(spec),
        Evm2Db::new(Evm2DatabaseRef::new(evm.db_mut())),
        precompiles_for_spec(spec),
    )
}

/// Converts a recovered reth Ethereum transaction into evm2's transaction envelope.
pub fn recovered_tx_to_evm2(tx: Recovered<&RethEthereumTxEnvelope>) -> RecoveredTxEnvelope {
    match tx.inner() {
        EthereumTxEnvelope::Legacy(inner) => {
            RecoveredTxEnvelope::Legacy(Recovered::new_unchecked(inner.tx().clone(), tx.signer()))
        }
        EthereumTxEnvelope::Eip2930(inner) => {
            RecoveredTxEnvelope::Eip2930(Recovered::new_unchecked(inner.tx().clone(), tx.signer()))
        }
        EthereumTxEnvelope::Eip1559(inner) => {
            RecoveredTxEnvelope::Eip1559(Recovered::new_unchecked(inner.tx().clone(), tx.signer()))
        }
        EthereumTxEnvelope::Eip4844(inner) => RecoveredTxEnvelope::Eip4844(
            Recovered::new_unchecked(inner.tx().clone().into(), tx.signer()),
        ),
        EthereumTxEnvelope::Eip7702(inner) => {
            RecoveredTxEnvelope::Eip7702(Recovered::new_unchecked(inner.tx().clone(), tx.signer()))
        }
    }
}

/// Returns blob gas used by a reth Ethereum transaction.
pub const fn blob_gas_used(tx: &RethEthereumTxEnvelope) -> u64 {
    match tx {
        EthereumTxEnvelope::Eip4844(tx) => {
            tx.tx().blob_versioned_hashes.len() as u64 * DATA_GAS_PER_BLOB
        }
        EthereumTxEnvelope::Legacy(_) |
        EthereumTxEnvelope::Eip2930(_) |
        EthereumTxEnvelope::Eip1559(_) |
        EthereumTxEnvelope::Eip7702(_) => 0,
    }
}

/// Returns the transaction gas limit for a reth Ethereum transaction.
pub const fn transaction_gas_limit(tx: &RethEthereumTxEnvelope) -> u64 {
    match tx {
        EthereumTxEnvelope::Legacy(tx) => tx.tx().gas_limit,
        EthereumTxEnvelope::Eip2930(tx) => tx.tx().gas_limit,
        EthereumTxEnvelope::Eip1559(tx) => tx.tx().gas_limit,
        EthereumTxEnvelope::Eip4844(tx) => tx.tx().gas_limit,
        EthereumTxEnvelope::Eip7702(tx) => tx.tx().gas_limit,
    }
}

/// Converts an `evm2` bytecode value into the revm bytecode type used by reth state bundles.
pub fn bytecode_from_evm2(bytecode: Evm2Bytecode) -> Bytecode {
    if bytecode.is_eip7702() {
        Bytecode::new_eip7702(bytecode.eip7702_address().expect("checked eip7702 bytecode"))
    } else if bytecode.is_empty() {
        Bytecode::new()
    } else if let Some(jump_table) = bytecode.legacy_jump_table() {
        let jump_table =
            revm_bytecode::JumpTable::from_slice(jump_table.as_slice(), jump_table.len());
        // SAFETY: evm2 bytecode has already been analyzed and enforces the same padding and jump
        // table invariants as revm bytecode.
        unsafe { Bytecode::new_analyzed(bytecode.bytes().clone(), bytecode.len(), jump_table) }
    } else {
        Bytecode::new_raw(bytecode.original_bytes())
    }
}

/// Converts an `evm2` account info value into the revm account info type used by reth.
pub fn account_info_from_evm2(info: Evm2AccountInfo) -> AccountInfo {
    AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        account_id: None,
        code: info.code.map(bytecode_from_evm2),
    }
}

fn account_info_from_evm2_with_code(
    mut info: Evm2AccountInfo,
    code: &BTreeMap<B256, Evm2Bytecode>,
) -> AccountInfo {
    if info.code.is_none() &&
        let Some(bytecode) = code.get(&info.code_hash)
    {
        info.code = Some(bytecode.clone());
    }
    account_info_from_evm2(info)
}

/// Converts a revm bytecode value into evm2 bytecode.
pub fn bytecode_to_evm2(bytecode: Bytecode) -> Evm2Bytecode {
    if bytecode.is_eip7702() {
        Evm2Bytecode::new_eip7702(bytecode.eip7702_address().expect("checked eip7702 bytecode"))
    } else if bytecode.is_empty() {
        Evm2Bytecode::new()
    } else if let Some(jump_table) = bytecode.legacy_jump_table() {
        let jump_table = Evm2JumpTable::from_slice(jump_table.as_slice(), jump_table.len());
        // SAFETY: revm bytecode has already been analyzed and enforces the same padding and jump
        // table invariants as evm2 bytecode.
        unsafe { Evm2Bytecode::new_analyzed(bytecode.bytes(), bytecode.len(), jump_table) }
    } else {
        Evm2Bytecode::new_raw(bytecode.original_bytes())
    }
}

/// Converts a revm account info value into evm2 account info.
pub fn account_info_to_evm2(info: AccountInfo) -> Evm2AccountInfo {
    Evm2AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: info.code.map(bytecode_to_evm2),
        _non_exhaustive: (),
    }
}

fn storage_from_evm2(storage: StorageChangeSet) -> StorageWithOriginalValues {
    storage
        .slots
        .into_iter()
        .map(|(key, slot)| (key, StorageSlot::new_changed(slot.original, slot.current)))
        .collect()
}

fn storage_reverts_from_evm2(storage: &StorageChangeSet) -> StorageKeyMap<RevertToSlot> {
    storage.slots.iter().map(|(&key, slot)| (key, RevertToSlot::Some(slot.original))).collect()
}

fn account_info_revert_from_evm2(
    original: &Option<Evm2AccountInfo>,
    present: &Option<Evm2AccountInfo>,
) -> AccountInfoRevert {
    if original.is_none() && present.is_none() {
        return AccountInfoRevert::DeleteIt;
    }
    let original = original.clone().map(account_info_from_evm2);
    let present = present.clone().map(account_info_from_evm2);
    account_info_revert(&original, &present)
}

fn account_info_revert(
    original: &Option<AccountInfo>,
    present: &Option<AccountInfo>,
) -> AccountInfoRevert {
    match (original, present) {
        (Some(original), Some(present)) if account_info_persistent_eq(original, present) => {
            AccountInfoRevert::DoNothing
        }
        (Some(original), None) if original.is_empty() => AccountInfoRevert::DoNothing,
        (Some(original), _) => AccountInfoRevert::RevertTo(original.clone()),
        (None, Some(_)) => AccountInfoRevert::DeleteIt,
        (None, None) => AccountInfoRevert::DoNothing,
    }
}

fn account_info_persistent_eq(a: &AccountInfo, b: &AccountInfo) -> bool {
    a.balance == b.balance && a.nonce == b.nonce && a.code_hash == b.code_hash
}

fn account_info_revert_from_bundle_account(account: &BundleAccount) -> AccountInfoRevert {
    if account.original_info.is_none() &&
        account.info.is_none() &&
        (account.status == AccountStatus::InMemoryChange || account.was_destroyed())
    {
        return AccountInfoRevert::DeleteIt;
    }
    account_info_revert(&account.original_info, &account.info)
}

const fn account_status(
    original: &Option<AccountInfo>,
    present: &Option<AccountInfo>,
    storage_wiped: bool,
) -> AccountStatus {
    if present.is_none() && original.is_some() {
        AccountStatus::Destroyed
    } else if original.is_none() {
        AccountStatus::InMemoryChange
    } else if storage_wiped {
        AccountStatus::DestroyedChanged
    } else {
        AccountStatus::Changed
    }
}

fn bundle_account_created_in_block(account: &BundleAccount) -> bool {
    account.original_info.is_none() && (account.info.is_some() || !account.storage.is_empty())
}

fn bundle_account_deleted_after_block_creation(
    account: &BundleAccount,
    storage: &StorageChangeSet,
) -> bool {
    account.original_info.is_none() &&
        account.info.is_none() &&
        account.was_destroyed() &&
        bundle_account_has_nonzero_storage_after(account, storage)
}

fn bundle_account_has_nonzero_storage_after(
    account: &BundleAccount,
    storage: &StorageChangeSet,
) -> bool {
    account.storage.iter().any(|(key, slot)| {
        storage
            .slots
            .get(key)
            .map_or_else(|| !slot.present_value.is_zero(), |slot| !slot.current.is_zero())
    }) || storage
        .slots
        .iter()
        .any(|(key, slot)| !account.storage.contains_key(key) && !slot.current.is_zero())
}

/// Converts a single `evm2` transaction or system-call state change set into a revm bundle.
pub fn bundle_state_from_evm2(changes: StateChanges) -> BundleState {
    let mut state = AddressMap::<BundleAccount>::default();
    let mut contracts = B256Map::<Bytecode>::default();
    let mut reverts = Vec::with_capacity(changes.accounts.len() + changes.storage.len());
    let mut reverts_size = 0;

    for (address, account) in &changes.accounts {
        let storage = changes.storage.get(address);
        let revert = AccountRevert {
            account: account_info_revert_from_evm2(&account.original, &account.current),
            storage: storage.map(storage_reverts_from_evm2).unwrap_or_default(),
            previous_status: AccountStatus::Changed,
            wipe_storage: storage.is_some_and(|storage| storage.wipe),
        };
        reverts_size += revert.size_hint();
        reverts.push((*address, revert));
    }

    for (address, storage) in &changes.storage {
        if changes.accounts.contains_key(address) {
            continue;
        }
        let revert = AccountRevert {
            account: AccountInfoRevert::DoNothing,
            storage: storage_reverts_from_evm2(storage),
            previous_status: AccountStatus::Changed,
            wipe_storage: storage.wipe,
        };
        reverts_size += revert.size_hint();
        reverts.push((*address, revert));
    }

    for (hash, bytecode) in changes.code {
        contracts.insert(hash, bytecode_from_evm2(bytecode));
    }

    let mut storage = changes.storage;
    let mut state_size = 0;
    for (address, account) in changes.accounts {
        let original = account.original.map(account_info_from_evm2);
        let present = account.current.map(account_info_from_evm2);
        let storage = storage.remove(&address);
        let storage_wiped = storage.as_ref().is_some_and(|storage| storage.wipe);
        let storage = storage.map(storage_from_evm2).unwrap_or_default();
        let status = account_status(&original, &present, storage_wiped);
        let account = BundleAccount::new(original, present, storage, status);
        state_size += account.size_hint();
        state.insert(address, account);
    }

    for (address, storage) in storage {
        let status = if storage.wipe { AccountStatus::Destroyed } else { AccountStatus::Changed };
        let storage = storage_from_evm2(storage);
        let account = BundleAccount::new(None, None, storage, status);
        state_size += account.size_hint();
        state.insert(address, account);
    }

    BundleState { state, contracts, reverts: Reverts::new(vec![reverts]), state_size, reverts_size }
}

fn block_reverts_from_bundle(bundle: &BundleState) -> (Vec<(Address, AccountRevert)>, usize) {
    let mut reverts = Vec::with_capacity(bundle.state.len());
    let mut reverts_size = 0;

    for (&address, account) in &bundle.state {
        let revert = AccountRevert {
            account: account_info_revert_from_bundle_account(account),
            storage: account
                .storage
                .iter()
                .filter(|(_, slot)| account.was_destroyed() || slot.is_changed())
                .map(|(&key, slot)| (key, RevertToSlot::Some(slot.original_value())))
                .collect(),
            previous_status: AccountStatus::Changed,
            wipe_storage: account.was_destroyed(),
        };
        reverts_size += revert.size_hint();
        reverts.push((address, revert));
    }

    (reverts, reverts_size)
}

fn into_block_bundle(mut bundle: BundleState) -> BundleState {
    let (reverts, reverts_size) = block_reverts_from_bundle(&bundle);
    bundle.reverts = Reverts::new(vec![reverts]);
    bundle.reverts_size = reverts_size;
    bundle
}

fn evm_state_from_evm2_with_accounts<R>(
    executor: &mut Evm2TransactionExecutor<R>,
    changes: StateChanges,
) -> Result<EvmState, BlockExecutionError>
where
    R: Evm2ReceiptBuilder,
{
    let mut state = AddressMap::<RevmAccount>::default();
    let StateChanges { accounts, storage, code, .. } = changes;
    let mut storage = storage;

    for (address, account) in accounts {
        let original = account.original.map(account_info_from_evm2);
        let is_created = original.is_none() && account.current.is_some();
        let storage_wiped = storage.get(&address).is_some_and(|storage| storage.wipe);
        let is_deleted_after_block_creation = account.current.is_none() &&
            executor
                .output
                .bundle
                .account(&address)
                .is_some_and(bundle_account_created_in_block);
        if (original.is_none() && account.current.is_none()) || is_deleted_after_block_creation {
            storage.remove(&address);
            let mut account = RevmAccount::new_not_existing(TransactionId::ZERO);
            account.mark_touch();
            account.mark_created();
            state.insert(address, account);
            continue;
        }
        let mut revm_account = match account.current {
            Some(info) => {
                let mut account = if is_created || storage_wiped {
                    RevmAccount::new_not_existing(TransactionId::ZERO)
                } else {
                    RevmAccount::default()
                };
                account.info = account_info_from_evm2_with_code(info, &code);
                account.transaction_id = TransactionId::ZERO;
                account
            }
            None => {
                let mut account = RevmAccount::new_not_existing(TransactionId::ZERO);
                account.mark_selfdestruct();
                account
            }
        };
        revm_account.mark_touch();
        if is_created || storage_wiped {
            revm_account.mark_created();
        }
        if let Some(original) = original {
            *revm_account.original_info_mut() = original;
        }
        if let Some(storage) = storage.remove(&address) {
            revm_account.storage = storage
                .slots
                .into_iter()
                .map(|(key, slot)| {
                    (
                        key,
                        EvmStorageSlot::new_changed(
                            slot.original,
                            slot.current,
                            TransactionId::ZERO,
                        ),
                    )
                })
                .collect();
        }
        state.insert(address, revm_account);
    }

    for (address, storage) in storage {
        let is_deleted_after_block_creation =
            executor.output.bundle.account(&address).is_some_and(|account| {
                bundle_account_deleted_after_block_creation(account, &storage)
            });
        if is_deleted_after_block_creation {
            let mut account = RevmAccount::new_not_existing(TransactionId::ZERO);
            account.mark_touch();
            account.mark_created();
            account.storage = storage
                .slots
                .into_iter()
                .map(|(key, slot)| {
                    (
                        key,
                        EvmStorageSlot::new_changed(
                            slot.original,
                            slot.current,
                            TransactionId::ZERO,
                        ),
                    )
                })
                .collect();
            state.insert(address, account);
            continue;
        }
        let current_info = executor.current_account_info(address).map_err(evm2_block_error)?;
        let is_storage_only_block_creation = current_info.is_none() &&
            !storage.slots.is_empty() &&
            storage.slots.values().all(|slot| slot.original.is_zero());
        let storage_wiped_existing_account = storage.wipe && current_info.is_some();
        let mut account = if is_storage_only_block_creation || storage_wiped_existing_account {
            RevmAccount::new_not_existing(TransactionId::ZERO)
        } else {
            RevmAccount::default()
        };
        if let Some(info) = current_info {
            account.info = info;
        }
        if is_storage_only_block_creation || storage_wiped_existing_account {
            account.mark_created();
        }
        account.mark_touch();
        if storage.wipe && !is_storage_only_block_creation && !storage_wiped_existing_account {
            account.mark_selfdestruct();
        }
        account.storage = storage
            .slots
            .into_iter()
            .map(|(key, slot)| {
                (key, EvmStorageSlot::new_changed(slot.original, slot.current, TransactionId::ZERO))
            })
            .collect();
        state.insert(address, account);
    }

    Ok(state)
}

fn evm_state_from_balance_increments<R>(
    executor: &mut Evm2TransactionExecutor<R>,
    increments: AddressMap<U256>,
) -> Result<EvmState, BlockExecutionError>
where
    R: Evm2ReceiptBuilder,
{
    let mut state = AddressMap::<RevmAccount>::default();

    for (address, amount) in increments {
        let mut account = RevmAccount::default();
        if let Some(info) = executor.current_account_info(address).map_err(evm2_block_error)? {
            account.info = info;
        }
        account.info.balance = account.info.balance.wrapping_add(amount);
        account.mark_touch();
        state.insert(address, account);
    }

    Ok(state)
}

/// Accumulates `evm2` execution output into reth's bundle-state shape.
#[derive(Debug, Default)]
pub struct Evm2ExecutionOutput {
    /// Accumulated state bundle.
    pub bundle: BundleState,
}

impl Evm2ExecutionOutput {
    /// Merges one transaction or system-call state change set into the accumulated bundle.
    pub fn merge_state_changes(&mut self, changes: StateChanges) {
        self.bundle.extend(bundle_state_from_evm2(changes));
    }

    /// Merges multiple state change sets in order.
    pub fn merge_state_changes_iter(&mut self, changes: impl IntoIterator<Item = StateChanges>) {
        for changes in changes {
            self.merge_state_changes(changes);
        }
    }

    /// Consumes the output accumulator and returns the merged bundle state.
    pub fn into_bundle(self) -> BundleState {
        into_block_bundle(self.bundle)
    }

    /// Takes a built EIP-7928 block access list.
    pub fn take_bal(&mut self) -> Option<BlockAccessList> {
        todo!("evm2 bals")
    }
}

/// Receipt builder for direct evm2 transaction execution.
pub trait Evm2ReceiptBuilder {
    /// Transaction type this builder accepts.
    type Transaction;
    /// Receipt type produced by this builder.
    type Receipt;

    /// Builds a receipt from an evm2 transaction result.
    fn build_receipt(
        &self,
        tx_type: TxType,
        result: Evm2TxResult,
        cumulative_gas_used: u64,
    ) -> Self::Receipt;
}

/// Reth Ethereum receipt builder for direct evm2 execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct RethEvm2ReceiptBuilder;

impl Evm2ReceiptBuilder for RethEvm2ReceiptBuilder {
    type Transaction = RethEthereumTxEnvelope;
    type Receipt = EthereumReceipt<TxType>;

    fn build_receipt(
        &self,
        tx_type: TxType,
        result: Evm2TxResult,
        cumulative_gas_used: u64,
    ) -> Self::Receipt {
        EthereumReceipt {
            tx_type,
            success: result.status,
            cumulative_gas_used,
            logs: result.state_changes.logs,
        }
    }
}

/// Result of direct evm2 transaction execution after merging state output.
#[derive(Debug)]
pub struct Evm2ExecutedTx<R> {
    /// Receipt for the executed transaction.
    pub receipt: R,
    /// Transaction gas used after refunds.
    pub gas_used: u64,
    /// Blob gas used by the transaction.
    pub blob_gas_used: u64,
}

/// Ommer header data needed for post-block balance increments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Evm2BlockOmmer {
    /// Ommer beneficiary.
    pub beneficiary: Address,
    /// Ommer block number.
    pub number: u64,
}

/// Ethereum block-level execution context for direct evm2 execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evm2BlockExecutionCtx<'a> {
    /// Parent block hash.
    pub parent_hash: B256,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// EIP-6110 deposit contract address override.
    pub deposit_contract_address: Option<Address>,
    /// Ommer headers for pre-Merge rewards.
    pub ommers: &'a [Evm2BlockOmmer],
    /// Withdrawals for EIP-4895 balance increments.
    pub withdrawals: Option<&'a [Withdrawal]>,
}

/// Error produced by direct evm2 block execution helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evm2BlockExecutionError {
    /// Transaction execution failed.
    Transaction(evm2::evm::registry::HandlerError),
    /// Transaction gas limit exceeds the gas still available in the block.
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction gas limit.
        transaction_gas_limit: u64,
        /// The gas still available in the block.
        block_available_gas: u64,
    },
    /// Cancun block is missing its parent beacon block root.
    MissingParentBeaconBlockRoot,
    /// Cancun genesis block has a non-zero parent beacon block root.
    CancunGenesisParentBeaconBlockRootNotZero {
        /// Supplied parent beacon block root.
        parent_beacon_block_root: B256,
    },
    /// System call execution failed.
    SystemCall {
        /// System call label.
        label: &'static str,
        /// System contract address.
        address: Address,
        /// Stop reason.
        stop: InstrStop,
    },
    /// System call execution failed with a database error.
    SystemCallDatabase {
        /// System call label.
        label: &'static str,
        /// System contract address.
        address: Address,
        /// Database error.
        error: String,
    },
    /// Database read failed while building block-level state output.
    Database(Evm2DbErrorCode),
    /// Deposit request log decoding failed.
    DepositRequestDecode,
}

impl From<evm2::evm::registry::HandlerError> for Evm2BlockExecutionError {
    fn from(error: evm2::evm::registry::HandlerError) -> Self {
        Self::Transaction(error)
    }
}

/// Result of direct evm2 block execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evm2BlockExecutionResult<R> {
    /// Transaction execution results in block order.
    pub transaction_results: Vec<Evm2TxResult>,
    /// Receipts for accepted transactions.
    pub receipts: Vec<R>,
    /// EIP-7685 requests emitted by block execution.
    pub requests: Requests,
    /// Total gas used by the block.
    pub gas_used: u64,
    /// Cumulative transaction gas used after refunds.
    pub cumulative_tx_gas_used: u64,
    /// Regular gas used by transactions in this block.
    pub block_regular_gas_used: u64,
    /// State gas used by transactions in this block.
    pub block_state_gas_used: u64,
    /// Blob gas used by transactions in the block.
    pub blob_gas_used: u64,
    /// Pre-block system call results.
    pub pre_block_system_results: Vec<Evm2TxResult>,
    /// Post-block system call results.
    pub post_block_system_results: Vec<Evm2TxResult>,
}

impl<R> Default for Evm2BlockExecutionResult<R> {
    fn default() -> Self {
        Self {
            transaction_results: Vec::new(),
            receipts: Vec::new(),
            requests: Requests::default(),
            gas_used: 0,
            cumulative_tx_gas_used: 0,
            block_regular_gas_used: 0,
            block_state_gas_used: 0,
            blob_gas_used: 0,
            pre_block_system_results: Vec::new(),
            post_block_system_results: Vec::new(),
        }
    }
}

impl<R> Evm2BlockExecutionResult<R> {
    /// Converts this evm2 block result into reth's block execution result shape.
    pub fn into_reth(self) -> BlockExecutionResult<R> {
        BlockExecutionResult {
            receipts: self.receipts,
            requests: self.requests,
            gas_used: self.gas_used,
            blob_gas_used: self.blob_gas_used,
        }
    }
}

/// Direct evm2 block executor that uses the concrete evm2 host.
pub type Evm2BlockExecutor<R = RethEvm2ReceiptBuilder> = Evm2TransactionExecutor<R>;

/// Factory for direct evm2 block executors.
#[derive(Debug, Clone)]
pub struct Evm2BlockExecutorFactory<R = RethEvm2ReceiptBuilder> {
    /// Receipt builder cloned into each executor.
    pub receipt_builder: R,
}

impl Evm2BlockExecutorFactory {
    /// Creates a new factory with reth's default Ethereum receipt builder.
    pub const fn new() -> Self {
        Self::with_receipt_builder(RethEvm2ReceiptBuilder)
    }
}

impl Default for Evm2BlockExecutorFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> Evm2BlockExecutorFactory<R> {
    /// Creates a new factory with the given receipt builder.
    pub const fn with_receipt_builder(receipt_builder: R) -> Self {
        Self { receipt_builder }
    }

    /// Creates a direct evm2 block executor from evm2 environment values.
    pub fn create_executor<DB>(
        &self,
        db: DB,
        spec: SpecId,
        block: Evm2BlockEnv,
    ) -> Evm2BlockExecutor<R>
    where
        DB: crate::Database + Send + 'static,
        R: Evm2ReceiptBuilder + Clone,
    {
        Evm2BlockExecutor::with_receipt_builder(
            create_evm2(db, spec, block),
            self.receipt_builder.clone(),
        )
    }

    /// Creates a direct evm2 block executor from revm environment values.
    pub fn create_executor_from_revm_env<DB>(
        &self,
        db: DB,
        spec: RevmSpecId,
        block: BlockEnv,
    ) -> Evm2BlockExecutor<R>
    where
        DB: crate::Database + Send + 'static,
        R: Evm2ReceiptBuilder + Clone,
    {
        self.create_executor(db, spec_id_from_revm(spec), block_env_from_revm(block))
    }
}

/// Alloy block executor factory backed by evm2 transaction execution.
#[derive(Debug, Clone)]
pub struct Evm2AlloyBlockExecutorFactory<R, Spec, EvmFactory> {
    /// Receipt builder.
    receipt_builder: R,
    /// Chain specification.
    spec: Spec,
    /// EVM factory used to create the carrier EVM.
    evm_factory: EvmFactory,
    /// DAO hardfork activation block, if configured.
    dao_fork_block: Option<u64>,
}

impl<R, Spec, EvmFactory> Evm2AlloyBlockExecutorFactory<R, Spec, EvmFactory> {
    /// Creates a new evm2-backed block executor factory.
    pub const fn new(receipt_builder: R, spec: Spec, evm_factory: EvmFactory) -> Self {
        Self { receipt_builder, spec, evm_factory, dao_fork_block: None }
    }

    /// Sets the DAO hardfork activation block for this factory.
    pub const fn with_dao_fork_block(mut self, dao_fork_block: Option<u64>) -> Self {
        self.dao_fork_block = dao_fork_block;
        self
    }

    /// Exposes the receipt builder.
    pub const fn receipt_builder(&self) -> &R {
        &self.receipt_builder
    }

    /// Exposes the chain specification.
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }
}

impl<R, Spec, EvmF> AlloyBlockExecutorFactory for Evm2AlloyBlockExecutorFactory<R, Spec, EvmF>
where
    R: Evm2ReceiptBuilder<Transaction = RethEthereumTxEnvelope> + Clone + Send + Sync + 'static,
    R::Receipt: Clone + Send + Sync + 'static,
    Spec: Send + Sync + 'static,
    EvmF: AlloyEvmFactory<
            Tx: FromRecoveredTx<RethEthereumTxEnvelope> + FromTxWithEncoded<RethEthereumTxEnvelope>,
            HaltReason = HaltReason,
            Spec = RevmSpecId,
            BlockEnv = BlockEnv,
            Precompiles = PrecompilesMap,
        > + Send
        + Sync
        + 'static,
{
    type EvmFactory = EvmF;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = RethEthereumTxEnvelope;
    type Receipt = R::Receipt;
    type TxExecutionResult = Evm2AlloyTxResult;
    type Executor<'a, DB: StateDB, I: Inspector<EvmF::Context<DB>>> =
        Evm2AlloyBlockExecutor<'a, EvmF::Evm<DB, I>, R>;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EvmF::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<EvmF::Context<DB>>,
    {
        Evm2AlloyBlockExecutor::new(evm, ctx, self.receipt_builder.clone(), self.dao_fork_block)
    }
}

/// A transaction result adapter for callers expecting alloy/revm result access.
#[derive(Debug, Clone)]
pub struct Evm2AlloyTxResult {
    result: ResultAndState<HaltReason>,
    evm2_result: Evm2TxResult,
    evm_state: EvmState,
    blob_gas_used: u64,
    tx_type: TxType,
}

impl AlloyTxResult for Evm2AlloyTxResult {
    type HaltReason = HaltReason;

    fn result(&self) -> &ResultAndState<Self::HaltReason> {
        &self.result
    }

    fn into_result(self) -> ResultAndState<Self::HaltReason> {
        self.result
    }
}

/// Alloy-compatible block executor that executes with evm2 and keeps reth's normal DB carrier.
pub struct Evm2AlloyBlockExecutor<'a, E, R: Evm2ReceiptBuilder> {
    evm2: Evm2TransactionExecutor<R>,
    evm: Box<E>,
    ctx: EthBlockExecutionCtx<'a>,
    dao_fork_block: Option<u64>,
}

impl<E, R> fmt::Debug for Evm2AlloyBlockExecutor<'_, E, R>
where
    E: fmt::Debug,
    R: Evm2ReceiptBuilder,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Evm2AlloyBlockExecutor")
            .field("evm", &self.evm)
            .field("ctx", &self.ctx)
            .finish_non_exhaustive()
    }
}

impl<'a, E, R> Evm2AlloyBlockExecutor<'a, E, R>
where
    E: AlloyEvm<Spec = RevmSpecId, BlockEnv = BlockEnv>,
    E::DB: crate::Database,
    R: Evm2ReceiptBuilder + Clone,
{
    /// Creates a new evm2-backed alloy block executor.
    pub fn new(
        evm: E,
        ctx: EthBlockExecutionCtx<'a>,
        receipt_builder: R,
        dao_fork_block: Option<u64>,
    ) -> Self {
        let mut evm = Box::new(evm);
        let evm2 = create_evm2_from_alloy_evm(&mut *evm);
        Self {
            evm2: Evm2TransactionExecutor::with_receipt_builder(evm2, receipt_builder),
            evm,
            ctx,
            dao_fork_block,
        }
    }
}

impl<E, R> AlloyBlockExecutor for Evm2AlloyBlockExecutor<'_, E, R>
where
    E: AlloyEvm<
        DB: crate::Database + DatabaseCommit,
        Tx: FromRecoveredTx<RethEthereumTxEnvelope> + FromTxWithEncoded<RethEthereumTxEnvelope>,
        HaltReason = HaltReason,
        Spec = RevmSpecId,
        BlockEnv = BlockEnv,
    >,
    R: Evm2ReceiptBuilder<Transaction = RethEthereumTxEnvelope> + Clone,
    R::Receipt: Clone + Send + 'static,
{
    type Transaction = RethEthereumTxEnvelope;
    type Receipt = R::Receipt;
    type Evm = E;
    type Result = Evm2AlloyTxResult;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        let start = self.evm2.result.pre_block_system_results.len();
        self.evm2
            .apply_pre_block_system_calls(self.ctx.parent_hash, self.ctx.parent_beacon_block_root)
            .map_err(evm2_block_error)?;
        let changes = self.evm2.result.pre_block_system_results[start..]
            .iter()
            .map(|result| result.state_changes.clone())
            .collect::<Vec<_>>();
        for changes in changes {
            self.evm.db_mut().commit(evm_state_from_evm2_with_accounts(&mut self.evm2, changes)?);
        }
        Ok(())
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (_, tx) = tx.into_parts();
        let recovered = Recovered::new_unchecked(tx.tx(), *tx.signer());
        let evm2_tx = recovered_tx_to_evm2(recovered);
        let transaction_gas_limit = transaction_gas_limit(tx.tx());
        let block_available_gas = self
            .evm2
            .evm
            .block_env()
            .gas_limit
            .saturating_sub(U256::from(self.evm2.result.gas_used))
            .saturating_to::<u64>();

        if transaction_gas_limit > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit,
                block_available_gas,
            }
            .into());
        }

        let evm2_result = self.evm2.evm.transact(&evm2_tx).map_err(|err| {
            BlockExecutionError::msg(format_args!("evm2 transaction execution failed: {err}"))
        })?;
        let evm_state =
            evm_state_from_evm2_with_accounts(&mut self.evm2, evm2_result.state_changes.clone())?;
        let result = evm2_result_to_revm_result(&evm2_result);

        Ok(Evm2AlloyTxResult {
            result,
            evm2_result,
            evm_state,
            blob_gas_used: blob_gas_used(tx.tx()),
            tx_type: tx.tx().tx_type(),
        })
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&Self::Result) -> CommitChanges,
    ) -> Result<Option<GasOutput>, BlockExecutionError> {
        let output = self.execute_transaction_without_commit(tx)?;
        if !f(&output).should_commit() {
            return Err(BlockExecutionError::msg(
                "evm2 transaction output cannot be discarded after execution",
            ));
        }

        Ok(Some(self.commit_transaction(output)))
    }

    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput {
        let Evm2AlloyTxResult { evm2_result, evm_state, blob_gas_used, tx_type, .. } = output;
        let gas_used = evm2_result.gas_used;

        self.evm2.result.cumulative_tx_gas_used =
            self.evm2.result.cumulative_tx_gas_used.saturating_add(gas_used);
        self.evm2.result.gas_used = self.evm2.result.gas_used.saturating_add(gas_used);
        self.evm2.result.block_regular_gas_used =
            self.evm2.result.block_regular_gas_used.saturating_add(gas_used);
        self.evm2.result.blob_gas_used =
            self.evm2.result.blob_gas_used.saturating_add(blob_gas_used);

        let receipt = self.evm2.receipt_builder.build_receipt(
            tx_type,
            evm2_result.clone(),
            self.evm2.result.cumulative_tx_gas_used,
        );
        self.evm.db_mut().commit(evm_state);
        self.evm2.output.merge_state_changes(evm2_result.state_changes.clone());
        self.evm2.result.transaction_results.push(evm2_result);
        self.evm2.result.receipts.push(receipt);

        GasOutput::with_state_gas(gas_used, 0)
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, AlloyBlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        if self.evm2.evm.spec_id().enables(SpecId::PRAGUE) {
            self.evm2
                .append_deposit_requests_from_tx_results(MAINNET_DEPOSIT_CONTRACT_ADDRESS)
                .map_err(evm2_block_error)?;
        }
        self.evm2.apply_post_block_system_calls().map_err(evm2_block_error)?;
        let ommers = eth_ommers_to_evm2(self.ctx.ommers);
        let mut balance_increments =
            self.evm2.post_block_balance_increments(&ommers, self.ctx.withdrawals.as_deref());
        if self.dao_fork_block.is_some_and(|dao_fork_block| {
            U256::from(dao_fork_block) == self.evm2.evm.block_env().number
        }) {
            let (drained_balance, drained_state) =
                self.evm2.drain_balances(dao_fork::DAO_HARDFORK_ACCOUNTS)?;
            self.evm.db_mut().commit(drained_state);
            *balance_increments.entry(dao_fork::DAO_HARDFORK_BENEFICIARY).or_default() +=
                drained_balance;
        }
        if !balance_increments.is_empty() {
            self.evm
                .db_mut()
                .commit(evm_state_from_balance_increments(&mut self.evm2, balance_increments)?);
        }
        self.evm2
            .apply_post_block_balance_increments(&ommers, self.ctx.withdrawals.as_deref())
            .map_err(evm2_block_error)?;

        let changes = self
            .evm2
            .result
            .post_block_system_results
            .iter()
            .map(|result| result.state_changes.clone())
            .collect::<Vec<_>>();
        for changes in changes {
            self.evm.db_mut().commit(evm_state_from_evm2_with_accounts(&mut self.evm2, changes)?);
        }

        let (_, result) = self.evm2.finish_evm2();

        Ok((
            *self.evm,
            AlloyBlockExecutionResult {
                receipts: result.receipts,
                requests: result.requests,
                gas_used: result.gas_used,
                blob_gas_used: result.blob_gas_used,
            },
        ))
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.evm2.result.receipts
    }
}

/// Direct evm2 block executor that accumulates reth execution output.
#[derive(Debug)]
pub struct Evm2TransactionExecutor<R: Evm2ReceiptBuilder = RethEvm2ReceiptBuilder> {
    /// Concrete evm2 host.
    pub evm: EthEvm2,
    /// Receipt builder.
    pub receipt_builder: R,
    /// Merged execution output.
    pub output: Evm2ExecutionOutput,
    /// Block execution result.
    pub result: Evm2BlockExecutionResult<R::Receipt>,
}

impl Evm2TransactionExecutor {
    /// Creates a direct evm2 transaction executor with the default receipt builder.
    pub fn new(evm: EthEvm2) -> Self {
        Self::with_receipt_builder(evm, RethEvm2ReceiptBuilder)
    }
}

impl<R> Evm2TransactionExecutor<R>
where
    R: Evm2ReceiptBuilder,
{
    /// Creates a direct evm2 transaction executor.
    pub fn with_receipt_builder(evm: EthEvm2, receipt_builder: R) -> Self {
        Self {
            evm,
            receipt_builder,
            output: Evm2ExecutionOutput::default(),
            result: Evm2BlockExecutionResult::default(),
        }
    }

    /// Executes one recovered Ethereum transaction, merges its output, and returns its receipt.
    pub fn execute_transaction(
        &mut self,
        tx: Recovered<&RethEthereumTxEnvelope>,
    ) -> Result<Evm2ExecutedTx<R::Receipt>, evm2::evm::registry::HandlerError>
    where
        R::Receipt: Clone,
    {
        let tx_type = tx.inner().tx_type();
        let blob_gas_used = blob_gas_used(tx.inner());
        let tx = recovered_tx_to_evm2(tx);
        let result = self.evm.transact(&tx)?;
        let gas_used = result.gas_used;
        self.result.cumulative_tx_gas_used =
            self.result.cumulative_tx_gas_used.saturating_add(gas_used);
        self.result.block_regular_gas_used =
            self.result.block_regular_gas_used.saturating_add(gas_used);
        self.result.gas_used = self.result.cumulative_tx_gas_used;
        self.result.blob_gas_used = self.result.blob_gas_used.saturating_add(blob_gas_used);
        let receipt = self.receipt_builder.build_receipt(
            tx_type,
            result.clone(),
            self.result.cumulative_tx_gas_used,
        );
        self.result.receipts.push(receipt.clone());
        self.output.merge_state_changes(result.state_changes.clone());
        self.result.transaction_results.push(result);
        Ok(Evm2ExecutedTx { receipt, gas_used, blob_gas_used })
    }

    /// Executes recovered Ethereum transactions in block order.
    pub fn execute_block_transactions<'a>(
        &mut self,
        txs: impl IntoIterator<Item = Recovered<&'a RethEthereumTxEnvelope>>,
    ) -> Result<(), Evm2BlockExecutionError>
    where
        R::Receipt: Clone,
    {
        for tx in txs {
            let tx_gas_limit = transaction_gas_limit(tx.inner());
            let block_available_gas =
                self.evm.block_env().gas_limit.saturating_sub(U256::from(self.result.gas_used));
            if U256::from(tx_gas_limit) > block_available_gas {
                return Err(Evm2BlockExecutionError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: tx_gas_limit,
                    block_available_gas: block_available_gas.saturating_to(),
                });
            }
            self.execute_transaction(tx)?;
        }
        Ok(())
    }

    /// Executes an Ethereum block using the direct evm2 host and reth-owned output merging.
    pub fn execute_block<'a>(
        &mut self,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        txs: impl IntoIterator<Item = Recovered<&'a RethEthereumTxEnvelope>>,
    ) -> Result<(), Evm2BlockExecutionError>
    where
        R::Receipt: Clone,
    {
        self.execute_block_with_context(
            Evm2BlockExecutionCtx { parent_hash, parent_beacon_block_root, ..Default::default() },
            txs,
        )
    }

    /// Executes an Ethereum block with an optional EIP-6110 deposit contract override.
    pub fn execute_block_with_deposit_contract<'a>(
        &mut self,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        deposit_contract_address: Option<Address>,
        txs: impl IntoIterator<Item = Recovered<&'a RethEthereumTxEnvelope>>,
    ) -> Result<(), Evm2BlockExecutionError>
    where
        R::Receipt: Clone,
    {
        self.execute_block_with_context(
            Evm2BlockExecutionCtx {
                parent_hash,
                parent_beacon_block_root,
                deposit_contract_address,
                ..Default::default()
            },
            txs,
        )
    }

    /// Executes an Ethereum block with explicit block-level execution context.
    pub fn execute_block_with_context<'a>(
        &mut self,
        ctx: Evm2BlockExecutionCtx<'a>,
        txs: impl IntoIterator<Item = Recovered<&'a RethEthereumTxEnvelope>>,
    ) -> Result<(), Evm2BlockExecutionError>
    where
        R::Receipt: Clone,
    {
        self.apply_pre_block_system_calls(ctx.parent_hash, ctx.parent_beacon_block_root)?;
        self.execute_block_transactions(txs)?;
        if self.evm.spec_id().enables(SpecId::PRAGUE) {
            self.append_deposit_requests_from_tx_results(
                ctx.deposit_contract_address.unwrap_or(MAINNET_DEPOSIT_CONTRACT_ADDRESS),
            )?;
        }
        self.apply_post_block_system_calls()?;
        self.apply_post_block_balance_increments(ctx.ommers, ctx.withdrawals)?;
        Ok(())
    }

    /// Appends EIP-6110 deposit requests parsed from evm2 transaction logs.
    pub fn append_deposit_requests_from_tx_results(
        &mut self,
        deposit_contract_address: Address,
    ) -> Result<(), Evm2BlockExecutionError> {
        let mut deposits = Vec::new();
        let logs = self
            .result
            .transaction_results
            .iter()
            .flat_map(|result| result.state_changes.logs.iter());
        alloy_evm::eth::eip6110::accumulate_deposits_from_logs(
            deposit_contract_address,
            logs,
            &mut deposits,
        )
        .map_err(|_| Evm2BlockExecutionError::DepositRequestDecode)?;
        if !deposits.is_empty() {
            self.result.requests.push_request_with_type(DEPOSIT_REQUEST_TYPE, deposits);
        }
        Ok(())
    }

    /// Applies pre-block EIP-2935 and EIP-4788 system calls.
    pub fn apply_pre_block_system_calls(
        &mut self,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
    ) -> Result<(), Evm2BlockExecutionError> {
        if self.evm.spec_id().enables(SpecId::PRAGUE) && !self.evm.block_env().number.is_zero() {
            self.execute_checked_system_call(
                "eip2935",
                HISTORY_STORAGE_ADDRESS,
                Bytes::copy_from_slice(parent_hash.as_slice()),
                SystemCallPhase::Pre,
            )?;
        }

        if self.evm.spec_id().enables(SpecId::CANCUN) {
            let parent_beacon_block_root = parent_beacon_block_root
                .ok_or(Evm2BlockExecutionError::MissingParentBeaconBlockRoot)?;
            if self.evm.block_env().number.is_zero() {
                if !parent_beacon_block_root.is_zero() {
                    return Err(
                        Evm2BlockExecutionError::CancunGenesisParentBeaconBlockRootNotZero {
                            parent_beacon_block_root,
                        },
                    );
                }
                return Ok(());
            }

            self.execute_checked_system_call(
                "eip4788",
                BEACON_ROOTS_ADDRESS,
                Bytes::copy_from_slice(parent_beacon_block_root.as_slice()),
                SystemCallPhase::Pre,
            )?;
        }

        Ok(())
    }

    /// Applies post-block EIP-7002 and EIP-7251 system calls.
    pub fn apply_post_block_system_calls(&mut self) -> Result<(), Evm2BlockExecutionError> {
        if !self.evm.spec_id().enables(SpecId::PRAGUE) {
            return Ok(());
        }

        let withdrawal = self.execute_checked_system_call(
            "eip7002",
            WITHDRAWAL_REQUEST_ADDRESS,
            Bytes::new(),
            SystemCallPhase::Post,
        )?;
        if !withdrawal.output.is_empty() {
            self.result.requests.push_request_with_type(WITHDRAWAL_REQUEST_TYPE, withdrawal.output);
        }

        let consolidation = self.execute_checked_system_call(
            "eip7251",
            CONSOLIDATION_REQUEST_ADDRESS,
            Bytes::new(),
            SystemCallPhase::Post,
        )?;
        if !consolidation.output.is_empty() {
            self.result
                .requests
                .push_request_with_type(CONSOLIDATION_REQUEST_TYPE, consolidation.output);
        }

        Ok(())
    }

    /// Calculates post-block balance increments for ommer rewards and withdrawals.
    pub fn post_block_balance_increments(
        &mut self,
        ommers: &[Evm2BlockOmmer],
        withdrawals: Option<&[Withdrawal]>,
    ) -> AddressMap<U256> {
        let mut balance_increments = AddressMap::with_capacity_and_hasher(
            withdrawals.map_or(ommers.len(), |withdrawals| withdrawals.len() + ommers.len() + 1),
            Default::default(),
        );

        if let Some(base_block_reward) = base_block_reward(self.evm.spec_id()) {
            let block_number = self.evm.block_env().number.saturating_to::<u64>();
            for ommer in ommers {
                *balance_increments.entry(ommer.beneficiary).or_default() +=
                    U256::from(ommer_reward(base_block_reward, block_number, ommer.number));
            }

            *balance_increments.entry(self.evm.block_env().beneficiary).or_default() +=
                U256::from(block_reward(base_block_reward, ommers.len()));
        }

        if self.evm.spec_id().enables(SpecId::SHANGHAI) &&
            let Some(withdrawals) = withdrawals
        {
            for withdrawal in withdrawals {
                let amount = withdrawal.amount_wei();
                if !amount.is_zero() {
                    *balance_increments.entry(withdrawal.address).or_default() += amount;
                }
            }
        }

        balance_increments
    }

    /// Applies post-block balance increments.
    pub fn apply_post_block_balance_increments(
        &mut self,
        ommers: &[Evm2BlockOmmer],
        withdrawals: Option<&[Withdrawal]>,
    ) -> Result<(), Evm2BlockExecutionError> {
        let increments = self.post_block_balance_increments(ommers, withdrawals);
        if increments.is_empty() {
            return Ok(());
        }

        let mut state = AddressMap::with_capacity_and_hasher(increments.len(), Default::default());
        let mut reverts = Vec::with_capacity(increments.len());
        let mut state_size = 0;
        let mut reverts_size = 0;

        for (address, amount) in increments {
            let original = self.current_account_info(address)?;
            let mut present = original.clone().unwrap_or_default();
            present.balance = present.balance.wrapping_add(amount);
            let present = Some(present);
            let status = account_status(&original, &present, false);
            let revert = AccountRevert {
                account: account_info_revert(&original, &present),
                storage: Default::default(),
                previous_status: AccountStatus::Changed,
                wipe_storage: false,
            };
            reverts_size += revert.size_hint();
            reverts.push((address, revert));

            let account =
                BundleAccount::new(original, present, StorageWithOriginalValues::default(), status);
            state_size += account.size_hint();
            state.insert(address, account);
        }

        self.output.bundle.extend(BundleState {
            state,
            reverts: Reverts::new(vec![reverts]),
            state_size,
            reverts_size,
            ..Default::default()
        });
        Ok(())
    }

    /// Drains balances from the given accounts and returns the total drained amount.
    pub fn drain_balances(
        &mut self,
        accounts: impl IntoIterator<Item = Address>,
    ) -> Result<(U256, EvmState), BlockExecutionError> {
        let mut drained_balance = U256::ZERO;
        let mut bundle_state = AddressMap::<BundleAccount>::default();
        let mut evm_state = AddressMap::<RevmAccount>::default();
        let mut reverts = Vec::new();
        let mut state_size = 0;
        let mut reverts_size = 0;

        for address in accounts {
            let Some(original) = self.current_account_info(address).map_err(evm2_block_error)?
            else {
                continue;
            };
            if original.balance.is_zero() {
                continue;
            }

            let mut present = original.clone();
            present.balance = U256::ZERO;
            drained_balance = drained_balance.saturating_add(original.balance);

            let mut revm_account = RevmAccount::default();
            revm_account.info = present.clone();
            revm_account.mark_touch();
            evm_state.insert(address, revm_account);

            let original = Some(original);
            let present = Some(present);
            let revert = AccountRevert {
                account: account_info_revert(&original, &present),
                storage: Default::default(),
                previous_status: AccountStatus::Changed,
                wipe_storage: false,
            };
            reverts_size += revert.size_hint();
            reverts.push((address, revert));

            let account = BundleAccount::new(
                original,
                present,
                StorageWithOriginalValues::default(),
                AccountStatus::Changed,
            );
            state_size += account.size_hint();
            bundle_state.insert(address, account);
        }

        self.output.bundle.extend(BundleState {
            state: bundle_state,
            reverts: Reverts::new(vec![reverts]),
            state_size,
            reverts_size,
            ..Default::default()
        });
        Ok((drained_balance, evm_state))
    }

    fn current_account_info(
        &mut self,
        address: Address,
    ) -> Result<Option<AccountInfo>, Evm2BlockExecutionError> {
        if let Some(account) = self.output.bundle.account(&address) {
            if account.info.is_some() {
                return Ok(account.info.clone());
            }
            if account.was_destroyed() {
                return Ok(None);
            }
        }
        if let Some(account) = self.evm.state().account_ref(&address) {
            return Ok(Some(account_info_from_evm2(account.info())));
        }
        self.evm
            .database_mut()
            .get_account(&address)
            .map(|info| info.map(account_info_from_evm2))
            .map_err(Evm2BlockExecutionError::Database)
    }

    fn execute_checked_system_call(
        &mut self,
        label: &'static str,
        address: Address,
        data: Bytes,
        phase: SystemCallPhase,
    ) -> Result<Evm2TxResult, Evm2BlockExecutionError> {
        let result = match phase {
            SystemCallPhase::Pre => self.execute_system_call(address, data),
            SystemCallPhase::Post => self.execute_post_block_system_call(address, data),
        };
        if !result.status {
            if let Some(code) = result.db_error_code {
                let error = self.evm.database_mut().error(code).to_string();
                return Err(Evm2BlockExecutionError::SystemCallDatabase { label, address, error });
            }
            return Err(Evm2BlockExecutionError::SystemCall { label, address, stop: result.stop });
        }
        Ok(result)
    }

    /// Executes a system call and merges the resulting state changes into the block output.
    pub fn execute_system_call(
        &mut self,
        system_contract_address: Address,
        data: Bytes,
    ) -> Evm2TxResult {
        let result = self.evm.system_call(system_contract_address, data);
        self.output.merge_state_changes(result.state_changes.clone());
        self.result.pre_block_system_results.push(result.clone());
        result
    }

    /// Executes a system call with an explicit caller and merges the resulting state changes.
    pub fn execute_system_call_with_caller(
        &mut self,
        caller: Address,
        system_contract_address: Address,
        data: Bytes,
    ) -> Evm2TxResult {
        let result = self.evm.system_call_with_caller(caller, system_contract_address, data);
        self.output.merge_state_changes(result.state_changes.clone());
        self.result.pre_block_system_results.push(result.clone());
        result
    }

    /// Records a post-block system call result and merges its state changes.
    pub fn execute_post_block_system_call(
        &mut self,
        system_contract_address: Address,
        data: Bytes,
    ) -> Evm2TxResult {
        let result = self.evm.system_call(system_contract_address, data);
        self.output.merge_state_changes(result.state_changes.clone());
        self.result.post_block_system_results.push(result.clone());
        result
    }

    /// Records a post-block system call result with an explicit caller and merges its state.
    pub fn execute_post_block_system_call_with_caller(
        &mut self,
        caller: Address,
        system_contract_address: Address,
        data: Bytes,
    ) -> Evm2TxResult {
        let result = self.evm.system_call_with_caller(caller, system_contract_address, data);
        self.output.merge_state_changes(result.state_changes.clone());
        self.result.post_block_system_results.push(result.clone());
        result
    }

    /// Finishes execution and returns reth's block execution output shape.
    pub fn finish(self) -> BlockExecutionOutput<R::Receipt> {
        BlockExecutionOutput { state: self.output.into_bundle(), result: self.result.into_reth() }
    }

    /// Finishes execution and returns both evm2's block result and reth's merged bundle state.
    pub fn finish_evm2(self) -> (BundleState, Evm2BlockExecutionResult<R::Receipt>) {
        (self.output.into_bundle(), self.result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemCallPhase {
    Pre,
    Post,
}

/// Converts block-level `evm2` transaction results into cumulative gas used values.
pub fn cumulative_gas_used<I>(gas_used: I) -> Vec<u64>
where
    I: IntoIterator<Item = u64>,
{
    let mut cumulative = 0u64;
    gas_used
        .into_iter()
        .map(|gas| {
            cumulative = cumulative.saturating_add(gas);
            cumulative
        })
        .collect()
}

/// Returns the block access list for an `evm2` execution output.
pub fn take_bal(_output: &mut Evm2ExecutionOutput) -> Option<BlockAccessList> {
    todo!("evm2 bals")
}

fn evm2_result_to_revm_result(result: &Evm2TxResult) -> ResultAndState<HaltReason> {
    let gas = ResultGas::new_with_state_gas(result.gas_used, 0, 0, 0);
    let logs = result.state_changes.logs.clone();
    let result = if result.status {
        ExecutionResult::Success {
            reason: SuccessReason::Return,
            gas,
            logs,
            output: Output::Call(result.output.clone()),
        }
    } else {
        ExecutionResult::Revert { gas, logs, output: result.output.clone() }
    };
    ResultAndState { result, state: EvmState::default() }
}

fn eth_ommers_to_evm2(ommers: &[alloy_consensus::Header]) -> Vec<Evm2BlockOmmer> {
    ommers
        .iter()
        .map(|ommer| Evm2BlockOmmer { beneficiary: ommer.beneficiary, number: ommer.number })
        .collect()
}

fn evm2_block_error(error: Evm2BlockExecutionError) -> BlockExecutionError {
    match error {
        Evm2BlockExecutionError::TransactionGasLimitMoreThanAvailableBlockGas {
            transaction_gas_limit,
            block_available_gas,
        } => BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
            transaction_gas_limit,
            block_available_gas,
        }
        .into(),
        Evm2BlockExecutionError::MissingParentBeaconBlockRoot => {
            BlockValidationError::MissingParentBeaconBlockRoot.into()
        }
        Evm2BlockExecutionError::CancunGenesisParentBeaconBlockRootNotZero {
            parent_beacon_block_root,
        } => BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
            parent_beacon_block_root,
        }
        .into(),
        Evm2BlockExecutionError::DepositRequestDecode => {
            BlockValidationError::DepositRequestDecode(Default::default()).into()
        }
        error => BlockExecutionError::msg(format_args!("evm2 block execution failed: {error:?}")),
    }
}

const fn base_block_reward(spec: SpecId) -> Option<u128> {
    if spec.enables(SpecId::MERGE) {
        None
    } else if spec.enables(SpecId::PETERSBURG) {
        Some(ONE_ETHER * 2)
    } else if spec.enables(SpecId::BYZANTIUM) {
        Some(ONE_ETHER * 3)
    } else {
        Some(ONE_ETHER * 5)
    }
}

const fn block_reward(base_block_reward: u128, ommers: usize) -> u128 {
    base_block_reward + (base_block_reward >> 5) * ommers as u128
}

const fn ommer_reward(base_block_reward: u128, block_number: u64, ommer_block_number: u64) -> u128 {
    ((8 + ommer_block_number - block_number) as u128 * base_block_reward) >> 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_eips::{eip4788::BEACON_ROOTS_CODE, Typed2718};
    use alloy_primitives::{address, Address, Bytes, Signature, TxKind, KECCAK256_EMPTY};
    use evm2::{
        evm::{precompile::PrecompileProvider, Tracked},
        interpreter::Host,
    };
    use revm::{
        database::{states::bundle_state::BundleRetention, CacheDB, EmptyDB, State as RevmState},
        Database, DatabaseCommit,
    };

    #[test]
    fn merges_evm2_account_storage_and_code_changes() {
        let address = address!("0x0000000000000000000000000000000000000001");
        let code = Evm2Bytecode::new_legacy(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: None,
                    current: Some(Evm2AccountInfo {
                        balance: U256::from(7),
                        nonce: 1,
                        code_hash,
                        code: Some(code.clone()),
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: false,
                    slots: core::iter::once((
                        U256::from(2),
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(3),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: core::iter::once((code_hash, code)).collect(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let account = bundle.account(&address).unwrap();

        assert_eq!(account.info.as_ref().unwrap().balance, U256::from(7));
        assert_eq!(account.storage_slot(U256::from(2)), Some(U256::from(3)));
        assert!(bundle.bytecode(&code_hash).is_some());

        let revert = &bundle.reverts[0][0].1;
        assert_eq!(revert.account, AccountInfoRevert::DeleteIt);
        assert_eq!(revert.storage.get(&U256::from(2)), Some(&RevertToSlot::Some(U256::ZERO)));
    }

    #[test]
    fn created_account_with_storage_wipe_is_not_destroyed() {
        let address = address!("0x0000000000000000000000000000000000000018");
        let code = Evm2Bytecode::new_legacy(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: None,
                    current: Some(Evm2AccountInfo {
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash,
                        code: Some(code.clone()),
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: true,
                    slots: core::iter::once((
                        U256::from(2),
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(3),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: core::iter::once((code_hash, code)).collect(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let account = bundle.account(&address).unwrap();

        assert_eq!(account.status, AccountStatus::InMemoryChange);
        assert!(!account.was_destroyed());
        assert_eq!(account.info.as_ref().map(|info| info.code_hash), Some(code_hash));
    }

    #[test]
    fn unchanged_account_info_revert_is_do_nothing() {
        let address = address!("0x000000000000000000000000000000000000001a");
        let code = Evm2Bytecode::new_legacy(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();
        let original = Evm2AccountInfo {
            balance: U256::from(1),
            nonce: 1,
            code_hash,
            code: None,
            _non_exhaustive: (),
        };
        let current = Evm2AccountInfo { code: Some(code), ..original.clone() };
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(original), current: Some(current), _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let revert = &bundle.reverts[0][0].1;

        assert_eq!(revert.account, AccountInfoRevert::DoNothing);
    }

    #[test]
    fn deleted_empty_account_revert_is_do_nothing() {
        let address = address!("0x000000000000000000000000000000000000001b");
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: Some(Evm2AccountInfo {
                        balance: U256::ZERO,
                        nonce: 0,
                        code_hash: KECCAK256_EMPTY,
                        code: Some(Evm2Bytecode::new()),
                        _non_exhaustive: (),
                    }),
                    current: None,
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet { wipe: true, slots: Default::default(), _non_exhaustive: () },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let revert = &bundle.reverts[0][0].1;

        assert_eq!(revert.account, AccountInfoRevert::DoNothing);
    }

    #[test]
    fn converts_destroyed_account_status() {
        let address = address!("0x0000000000000000000000000000000000000002");
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: Some(Evm2AccountInfo {
                        balance: U256::from(1),
                        nonce: 1,
                        code_hash: KECCAK256_EMPTY,
                        code: None,
                        _non_exhaustive: (),
                    }),
                    current: None,
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);

        assert!(bundle.account(&address).unwrap().was_destroyed());
    }

    #[test]
    fn preserves_created_deleted_account_revert() {
        let address = address!("0x000000000000000000000000000000000000000e");
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: None, current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let revert = &bundle.reverts[0][0].1;

        assert_eq!(revert.account, AccountInfoRevert::DeleteIt);
        assert_eq!(into_block_bundle(bundle).reverts[0][0].1.account, AccountInfoRevert::DeleteIt);
    }

    #[test]
    fn created_deleted_account_revm_state_prunes_revert_without_post_state() {
        let address = address!("0x000000000000000000000000000000000000000f");
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: None, current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let evm_state =
            evm_state_from_evm2_with_accounts(&mut executor, changes).expect("evm state converts");
        let mut db = RevmState::builder()
            .with_database(CacheDB::new(EmptyDB::default()))
            .with_bundle_update()
            .build();

        db.commit(evm_state);
        db.merge_transitions(BundleRetention::Reverts);
        crate::execute::prune_created_deleted_empty_accounts(&mut db.bundle_state);

        assert!(db.bundle_state.account(&address).is_none());
        assert!(db.bundle_state.reverts[0].iter().all(|(addr, _)| *addr != address));
    }

    #[test]
    fn created_deleted_account_prune_keeps_older_reverts() {
        let address = address!("0x0000000000000000000000000000000000000019");
        let revert = AccountRevert {
            account: AccountInfoRevert::DeleteIt,
            storage: Default::default(),
            previous_status: AccountStatus::Changed,
            wipe_storage: false,
        };
        let mut state = AddressMap::default();
        state.insert(
            address,
            BundleAccount::new(
                None,
                Some(AccountInfo::default()),
                Default::default(),
                AccountStatus::InMemoryChange,
            ),
        );
        let reverts_size = revert.size_hint() * 2;
        let mut bundle = BundleState {
            state,
            reverts: Reverts::new(vec![vec![(address, revert.clone())], vec![(address, revert)]]),
            state_size: 1,
            reverts_size,
            ..Default::default()
        };

        crate::execute::prune_created_deleted_empty_accounts(&mut bundle);

        assert_eq!(bundle.reverts[0].len(), 1);
        assert_eq!(bundle.reverts[1].len(), 0);
        assert!(bundle.account(&address).is_none());
    }

    #[test]
    fn evm2_output_keeps_block_original_revert_for_created_then_deleted_account() {
        let address = address!("0x0000000000000000000000000000000000000010");
        let created = Evm2AccountInfo {
            balance: U256::ZERO,
            nonce: 1,
            code_hash: KECCAK256_EMPTY,
            code: None,
            _non_exhaustive: (),
        };
        let mut output = Evm2ExecutionOutput::default();

        output.merge_state_changes(StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: None, current: Some(created.clone()), _non_exhaustive: () },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: true,
                    slots: core::iter::once((
                        U256::from(3),
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(5),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        });
        output.merge_state_changes(StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(created), current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet { wipe: true, slots: Default::default(), _non_exhaustive: () },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        });

        assert_eq!(
            into_block_bundle(output.bundle).reverts[0][0].1.account,
            AccountInfoRevert::DeleteIt
        );
    }

    #[test]
    fn evm2_revm_state_prunes_block_original_revert_for_created_then_deleted_account() {
        let address = address!("0x0000000000000000000000000000000000000011");
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let mut db = RevmState::builder()
            .with_database(CacheDB::new(EmptyDB::default()))
            .with_bundle_update()
            .build();

        let created_changes = StateChanges {
            accounts: Default::default(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: true,
                    slots: core::iter::once((
                        U256::from(3),
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(5),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };
        let deleted_changes = StateChanges {
            accounts: Default::default(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: true,
                    slots: core::iter::once((
                        U256::from(3),
                        Tracked {
                            original: U256::from(5),
                            current: U256::ZERO,
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        db.commit(
            evm_state_from_evm2_with_accounts(&mut executor, created_changes.clone())
                .expect("created state converts"),
        );
        executor.output.merge_state_changes(created_changes);
        db.commit(
            evm_state_from_evm2_with_accounts(&mut executor, deleted_changes)
                .expect("deleted state converts"),
        );
        db.merge_transitions(BundleRetention::Reverts);
        crate::execute::prune_created_deleted_empty_accounts(&mut db.bundle_state);

        assert!(
            db.bundle_state.account(&address).is_none(),
            "account={:?} reverts={:?}",
            db.bundle_state.account(&address),
            db.bundle_state.reverts
        );
        assert!(db.bundle_state.reverts[0].iter().all(|(addr, _)| *addr != address));
    }

    #[test]
    fn evm2_revm_state_marks_non_wipe_storage_after_block_creation_delete() {
        let address = address!("0x0000000000000000000000000000000000000015");
        let created = Evm2AccountInfo {
            balance: U256::from(1),
            nonce: 1,
            code_hash: KECCAK256_EMPTY,
            code: None,
            _non_exhaustive: (),
        };
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let mut db = RevmState::builder()
            .with_database(CacheDB::new(EmptyDB::default()))
            .with_bundle_update()
            .build();

        let created_changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: None, current: Some(created.clone()), _non_exhaustive: () },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: false,
                    slots: core::iter::once((
                        U256::from(3),
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(5),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };
        let deleted_changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(created), current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };
        let post_delete_storage = StateChanges {
            accounts: Default::default(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: false,
                    slots: core::iter::once((
                        U256::from(3),
                        Tracked {
                            original: U256::from(5),
                            current: U256::from(6),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        db.commit(
            evm_state_from_evm2_with_accounts(&mut executor, created_changes.clone())
                .expect("created state converts"),
        );
        executor.output.merge_state_changes(created_changes);
        db.commit(
            evm_state_from_evm2_with_accounts(&mut executor, deleted_changes.clone())
                .expect("deleted state converts"),
        );
        executor.output.merge_state_changes(deleted_changes);
        assert!(
            bundle_account_deleted_after_block_creation(
                executor.output.bundle.account(&address).expect("account exists"),
                post_delete_storage.storage.get(&address).expect("storage exists")
            ),
            "account={:?}",
            executor.output.bundle.account(&address)
        );
        db.commit(
            evm_state_from_evm2_with_accounts(&mut executor, post_delete_storage)
                .expect("post-delete storage state converts"),
        );
        db.merge_transitions(BundleRetention::Reverts);
        crate::execute::prune_created_deleted_empty_accounts(&mut db.bundle_state);

        assert!(
            db.bundle_state.account(&address).is_some(),
            "account={:?} reverts={:?}",
            db.bundle_state.account(&address),
            db.bundle_state.reverts
        );
        assert_eq!(db.bundle_state.reverts[0][0].1.account, AccountInfoRevert::DeleteIt);
    }

    #[test]
    fn evm2_created_deleted_storage_marker_requires_nonzero_effective_storage() {
        let mut storage = StorageWithOriginalValues::default();
        storage.insert(U256::from(3), StorageSlot::new_changed(U256::ZERO, U256::from(5)));
        let account = BundleAccount::new(None, None, storage, AccountStatus::DestroyedChanged);
        let zeroed_storage = StorageChangeSet {
            wipe: false,
            slots: core::iter::once((
                U256::from(3),
                Tracked { original: U256::from(5), current: U256::ZERO, _non_exhaustive: () },
            ))
            .collect(),
            _non_exhaustive: (),
        };
        let nonzero_storage = StorageChangeSet {
            wipe: false,
            slots: core::iter::once((
                U256::from(3),
                Tracked { original: U256::from(5), current: U256::from(6), _non_exhaustive: () },
            ))
            .collect(),
            _non_exhaustive: (),
        };

        assert!(!bundle_account_deleted_after_block_creation(&account, &zeroed_storage));
        assert!(bundle_account_deleted_after_block_creation(&account, &nonzero_storage));
    }

    #[test]
    fn converts_destroyed_account_original_info_for_state_hook() {
        let address = address!("0x000000000000000000000000000000000000000c");
        let original = Evm2AccountInfo {
            balance: U256::from(1),
            nonce: 1,
            code_hash: B256::with_last_byte(1),
            code: None,
            _non_exhaustive: (),
        };
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(original.clone()), current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);

        let state =
            evm_state_from_evm2_with_accounts(&mut executor, changes).expect("evm state converts");
        let account = state.get(&address).expect("account exists");

        assert!(account.is_selfdestructed());
        assert_eq!(account.original_info().code_hash, original.code_hash);
    }

    #[test]
    fn evm2_revm_state_wipes_surviving_account_storage() {
        let address = address!("0x0000000000000000000000000000000000000016");
        let stale_slot = U256::from(1);
        let new_slot = U256::from(2);
        let original = Evm2AccountInfo {
            balance: U256::from(1),
            nonce: 1,
            code_hash: KECCAK256_EMPTY,
            code: None,
            _non_exhaustive: (),
        };
        let current = Evm2AccountInfo { balance: U256::from(2), ..original.clone() };
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let mut db = RevmState::builder()
            .with_database(CacheDB::new(EmptyDB::default()))
            .with_bundle_update()
            .build();
        let mut stale_storage = StorageKeyMap::default();
        stale_storage.insert(stale_slot, U256::from(9));
        db.cache.insert_account_with_storage(
            address,
            account_info_from_evm2(original.clone()),
            stale_storage,
        );
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(original), current: Some(current), _non_exhaustive: () },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: true,
                    slots: core::iter::once((
                        new_slot,
                        Tracked {
                            original: U256::ZERO,
                            current: U256::from(7),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        db.commit(evm_state_from_evm2_with_accounts(&mut executor, changes).unwrap());

        assert_eq!(db.storage(address, stale_slot).unwrap(), U256::ZERO);
        assert_eq!(db.storage(address, new_slot).unwrap(), U256::from(7));
    }

    #[test]
    fn evm2_revm_state_attaches_changed_code() {
        let address = address!("0x0000000000000000000000000000000000000017");
        let code = Evm2Bytecode::new_legacy(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let mut db = RevmState::builder()
            .with_database(CacheDB::new(EmptyDB::default()))
            .with_bundle_update()
            .build();
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: None,
                    current: Some(Evm2AccountInfo {
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash,
                        code: None,
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: Default::default(),
            code: core::iter::once((code_hash, code)).collect(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        db.commit(evm_state_from_evm2_with_accounts(&mut executor, changes).unwrap());
        db.merge_transitions(BundleRetention::Reverts);

        assert!(db.bundle_state.bytecode(&code_hash).is_some());
    }

    #[test]
    fn converts_storage_only_changes_to_reverts() {
        let address = address!("0x0000000000000000000000000000000000000003");
        let changes = StateChanges {
            accounts: Default::default(),
            storage: core::iter::once((
                address,
                StorageChangeSet {
                    wipe: false,
                    slots: core::iter::once((
                        U256::from(2),
                        Tracked {
                            original: U256::from(1),
                            current: U256::from(3),
                            _non_exhaustive: (),
                        },
                    ))
                    .collect(),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let revert = &bundle.reverts[0][0];

        assert_eq!(revert.0, address);
        assert_eq!(revert.1.account, AccountInfoRevert::DoNothing);
        assert_eq!(revert.1.storage.get(&U256::from(2)), Some(&RevertToSlot::Some(U256::from(1))));
    }

    #[test]
    fn converts_storage_wipe_to_storage_known_status() {
        let address = address!("0x000000000000000000000000000000000000000d");
        let changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked {
                    original: None,
                    current: Some(Evm2AccountInfo {
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash: KECCAK256_EMPTY,
                        code: None,
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: core::iter::once((
                address,
                StorageChangeSet { wipe: true, slots: Default::default(), _non_exhaustive: () },
            ))
            .collect(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        let bundle = bundle_state_from_evm2(changes);
        let account = bundle.account(&address).unwrap();

        assert_eq!(account.status, AccountStatus::InMemoryChange);
        assert_eq!(account.storage_slot(U256::from(1)), Some(U256::ZERO));
    }

    #[test]
    fn cumulative_gas_saturates() {
        assert_eq!(cumulative_gas_used([1, 2, u64::MAX]), [1, 3, u64::MAX]);
    }

    #[test]
    fn reth_precompile_cache_returns_spec_provider() {
        let precompiles = precompiles_for_spec(SpecId::BERLIN);

        assert!(<Precompiles as PrecompileProvider<evm2::BaseEvmTypes>>::contains(
            &precompiles,
            &Address::with_last_byte(1),
        ));
    }

    #[test]
    fn creates_concrete_evm2_from_revm_env() {
        let mut evm =
            create_evm2_from_revm_env(EmptyDB::default(), RevmSpecId::PRAGUE, BlockEnv::default());

        assert_eq!(evm.spec_id(), SpecId::PRAGUE);
        assert_eq!(evm.block_env().number, U256::ZERO);
    }

    #[test]
    fn factory_creates_direct_evm2_block_executor() {
        let factory = Evm2BlockExecutorFactory::new();
        let mut executor = factory.create_executor_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::PRAGUE,
            BlockEnv { number: U256::from(3), ..Default::default() },
        );

        assert_eq!(executor.evm.spec_id(), SpecId::PRAGUE);
        assert_eq!(executor.evm.block_env().number, U256::from(3));
    }

    #[test]
    fn converts_recovered_reth_transaction_to_evm2() {
        let signer = address!("0x0000000000000000000000000000000000000003");
        let tx = TxLegacy { gas_limit: 21_000, ..Default::default() }
            .into_signed(Signature::test_signature())
            .into();
        let recovered = Recovered::new_unchecked(&tx, signer);

        let evm2_tx = recovered_tx_to_evm2(recovered);

        assert_eq!(evm2_tx.ty(), 0);
        assert_eq!(evm2_tx.as_legacy().unwrap().signer(), signer);
    }

    #[test]
    fn direct_evm2_executor_merges_transaction_output() {
        let signer = address!("0x0000000000000000000000000000000000000004");
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(signer, AccountInfo::default());
        let evm = create_evm2_from_revm_env(db, RevmSpecId::FRONTIER, BlockEnv::default());
        let mut executor = Evm2TransactionExecutor::new(evm);
        let tx = TxLegacy {
            gas_limit: 21_000,
            to: TxKind::Call(address!("0x0000000000000000000000000000000000000005")),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let executed = executor
            .execute_transaction(Recovered::new_unchecked(&tx, signer))
            .expect("transaction executes");

        assert_eq!(executed.gas_used, 21_000);
        assert_eq!(executed.receipt.cumulative_gas_used, 21_000);
        assert_eq!(executor.result.cumulative_tx_gas_used, 21_000);
        assert_eq!(executor.result.transaction_results.len(), 1);
        let output = executor.finish();
        assert_eq!(output.result.receipts.len(), 1);
        assert!(output.state.account(&signer).is_some());
    }

    #[test]
    fn direct_evm2_executor_merges_system_call_output() {
        let contract = address!("0x0000000000000000000000000000000000000006");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x07, 0x60, 0x00, 0x55, 0x00]));
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(contract, AccountInfo::default().with_code(code));
        let evm = create_evm2_from_revm_env(db, RevmSpecId::FRONTIER, BlockEnv::default());
        let mut executor = Evm2TransactionExecutor::new(evm);

        let result = executor.execute_system_call(contract, Bytes::new());

        assert!(result.status);
        assert_eq!(
            result
                .state_changes
                .storage
                .get(&contract)
                .and_then(|storage| storage.slots.get(&U256::ZERO))
                .map(|slot| slot.current),
            Some(U256::from(7))
        );
        let output = executor.finish();
        assert_eq!(
            output.state.account(&contract).and_then(|account| account.storage_slot(U256::ZERO)),
            Some(U256::from(7))
        );
    }

    #[test]
    fn direct_evm2_executor_exposes_block_execution_result() {
        let signer = address!("0x0000000000000000000000000000000000000007");
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(signer, AccountInfo::default());
        let evm = create_evm2_from_revm_env(db, RevmSpecId::FRONTIER, BlockEnv::default());
        let mut executor = Evm2TransactionExecutor::new(evm);
        let tx = TxLegacy {
            gas_limit: 21_000,
            to: TxKind::Call(address!("0x0000000000000000000000000000000000000008")),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        executor
            .execute_block_transactions([Recovered::new_unchecked(&tx, signer)])
            .expect("block transactions execute");

        let (state, result) = executor.finish_evm2();
        assert_eq!(result.transaction_results.len(), 1);
        assert_eq!(result.receipts.len(), 1);
        assert_eq!(result.gas_used, 21_000);
        assert_eq!(result.cumulative_tx_gas_used, 21_000);
        assert_eq!(result.block_regular_gas_used, 21_000);
        assert_eq!(result.blob_gas_used, 0);
        assert!(state.account(&signer).is_some());
    }

    #[test]
    fn direct_evm2_executor_rejects_tx_over_remaining_block_gas() {
        let signer = address!("0x000000000000000000000000000000000000000b");
        let block = BlockEnv { gas_limit: 30_000, ..Default::default() };
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(signer, AccountInfo::default());
        let evm = create_evm2_from_revm_env(db, RevmSpecId::FRONTIER, block);
        let mut executor = Evm2TransactionExecutor::new(evm);
        let tx = TxLegacy {
            gas_limit: 21_000,
            to: TxKind::Call(address!("0x000000000000000000000000000000000000000c")),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let error = executor
            .execute_block_transactions([
                Recovered::new_unchecked(&tx, signer),
                Recovered::new_unchecked(&tx, signer),
            ])
            .expect_err("second transaction exceeds remaining block gas");

        assert_eq!(
            error,
            Evm2BlockExecutionError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: 21_000,
                block_available_gas: 9_000,
            }
        );
        assert_eq!(executor.result.transaction_results.len(), 1);
    }

    #[test]
    fn direct_evm2_executor_calculates_pre_merge_balance_increments() {
        let beneficiary = address!("0x000000000000000000000000000000000000000e");
        let ommer_beneficiary = address!("0x000000000000000000000000000000000000000f");
        let block = BlockEnv { number: U256::from(10), beneficiary, ..Default::default() };
        let evm = create_evm2_from_revm_env(EmptyDB::default(), RevmSpecId::BYZANTIUM, block);
        let mut executor = Evm2TransactionExecutor::new(evm);

        let increments = executor.post_block_balance_increments(
            &[Evm2BlockOmmer { beneficiary: ommer_beneficiary, number: 9 }],
            None,
        );

        assert_eq!(
            increments.get(&beneficiary).copied(),
            Some(U256::from(3_093_750_000_000_000_000u128))
        );
        assert_eq!(
            increments.get(&ommer_beneficiary).copied(),
            Some(U256::from(2_625_000_000_000_000_000u128))
        );
    }

    #[test]
    fn direct_evm2_executor_calculates_withdrawal_balance_increments() {
        let recipient = address!("0x0000000000000000000000000000000000000010");
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::SHANGHAI,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let withdrawal = Withdrawal { index: 0, validator_index: 0, address: recipient, amount: 7 };

        let increments = executor.post_block_balance_increments(&[], Some(&[withdrawal]));

        assert_eq!(increments.get(&recipient).copied(), Some(U256::from(7_000_000_000u64)));
    }

    #[test]
    fn direct_evm2_executor_applies_withdrawal_balance_increment_output() {
        let recipient = address!("0x0000000000000000000000000000000000000011");
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::SHANGHAI,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        let withdrawal = Withdrawal { index: 0, validator_index: 0, address: recipient, amount: 1 };

        executor
            .apply_post_block_balance_increments(&[], Some(&[withdrawal]))
            .expect("balance increments apply");

        let (state, _) = executor.finish_evm2();
        let account = state.account(&recipient).expect("recipient account exists");
        assert_eq!(account.original_info, None);
        assert_eq!(account.info.as_ref().map(|info| info.balance), Some(U256::from(1_000_000_000)));
        assert_eq!(state.reverts[0][0].0, recipient);
        assert_eq!(state.reverts[0][0].1.account, AccountInfoRevert::DeleteIt);
    }

    #[test]
    fn direct_evm2_executor_preserves_original_info_for_balance_increment_output() {
        let recipient = address!("0x0000000000000000000000000000000000000012");
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::SHANGHAI,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        executor.output.merge_state_changes(StateChanges {
            accounts: core::iter::once((
                recipient,
                Tracked {
                    original: Some(Evm2AccountInfo {
                        balance: U256::from(5),
                        nonce: 0,
                        code_hash: KECCAK256_EMPTY,
                        code: None,
                        _non_exhaustive: (),
                    }),
                    current: Some(Evm2AccountInfo {
                        balance: U256::from(7),
                        nonce: 0,
                        code_hash: KECCAK256_EMPTY,
                        code: None,
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        });
        let withdrawal = Withdrawal { index: 0, validator_index: 0, address: recipient, amount: 1 };

        executor
            .apply_post_block_balance_increments(&[], Some(&[withdrawal]))
            .expect("balance increments apply");

        let (state, _) = executor.finish_evm2();
        let account = state.account(&recipient).expect("recipient account exists");
        assert_eq!(account.original_info.as_ref().map(|info| info.balance), Some(U256::from(5)));
        assert_eq!(account.info.as_ref().map(|info| info.balance), Some(U256::from(1_000_000_007)));
        assert_eq!(state.reverts.len(), 1);
        assert_eq!(state.reverts[0][0].0, recipient);
        assert_eq!(
            state.reverts[0][0].1.account,
            AccountInfoRevert::RevertTo(AccountInfo {
                balance: U256::from(5),
                ..Default::default()
            })
        );
    }

    #[test]
    fn direct_evm2_executor_applies_withdrawal_to_created_account_output() {
        let recipient = address!("0x0000000000000000000000000000000000000013");
        let code = Evm2Bytecode::new_legacy(Bytes::from_static(&[0x60, 0x00, 0x00]));
        let code_hash = code.hash_slow();
        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::SHANGHAI,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        executor.output.merge_state_changes(StateChanges {
            accounts: core::iter::once((
                recipient,
                Tracked {
                    original: None,
                    current: Some(Evm2AccountInfo {
                        balance: U256::from(7),
                        nonce: 1,
                        code_hash,
                        code: Some(code.clone()),
                        _non_exhaustive: (),
                    }),
                    _non_exhaustive: (),
                },
            ))
            .collect(),
            storage: Default::default(),
            code: core::iter::once((code_hash, code)).collect(),
            logs: Vec::new(),
            _non_exhaustive: (),
        });
        let withdrawal = Withdrawal { index: 0, validator_index: 0, address: recipient, amount: 1 };

        executor
            .apply_post_block_balance_increments(&[], Some(&[withdrawal]))
            .expect("balance increments apply");

        let (state, _) = executor.finish_evm2();
        let account = state.account(&recipient).expect("recipient account exists");
        assert_eq!(account.status, AccountStatus::InMemoryChange);
        assert_eq!(account.original_info, None);
        assert_eq!(account.info.as_ref().map(|info| info.balance), Some(U256::from(1_000_000_007)));
        assert_eq!(account.info.as_ref().map(|info| info.nonce), Some(1));
        assert_eq!(account.info.as_ref().map(|info| info.code_hash), Some(code_hash));
        assert!(state.bytecode(&code_hash).is_some());
    }

    #[test]
    fn direct_evm2_executor_drains_balances_with_reverts() {
        let address = address!("0x0000000000000000000000000000000000000015");
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            address,
            AccountInfo { balance: U256::from(5), ..Default::default() },
        );
        let evm = create_evm2_from_revm_env(db, RevmSpecId::FRONTIER, BlockEnv::default());
        let mut executor = Evm2TransactionExecutor::new(evm);

        let (drained, _) = executor.drain_balances([address]).expect("balances drain");
        let (state, _) = executor.finish_evm2();

        assert_eq!(drained, U256::from(5));
        assert_eq!(state.account(&address).unwrap().info.as_ref().unwrap().balance, U256::ZERO);
        assert_eq!(state.reverts[0][0].0, address);
        assert_eq!(
            state.reverts[0][0].1.account,
            AccountInfoRevert::RevertTo(AccountInfo {
                balance: U256::from(5),
                ..Default::default()
            })
        );
    }

    #[test]
    fn evm2_output_merges_destroy_then_pre_spurious_empty_resurrection() {
        let address = address!("0x0000000000000000000000000000000000000014");
        let destroyed = Evm2AccountInfo {
            balance: U256::ZERO,
            nonce: 0,
            code_hash: B256::with_last_byte(1),
            code: None,
            _non_exhaustive: (),
        };
        let empty = Evm2AccountInfo {
            balance: U256::ZERO,
            nonce: 0,
            code_hash: KECCAK256_EMPTY,
            code: None,
            _non_exhaustive: (),
        };
        let mut output = Evm2ExecutionOutput::default();
        let resurrection_changes = StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: None, current: Some(empty), _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        };

        output.merge_state_changes(StateChanges {
            accounts: core::iter::once((
                address,
                Tracked { original: Some(destroyed.clone()), current: None, _non_exhaustive: () },
            ))
            .collect(),
            storage: Default::default(),
            code: Default::default(),
            logs: Vec::new(),
            _non_exhaustive: (),
        });
        output.merge_state_changes(resurrection_changes.clone());

        let account = output.bundle.account(&address).expect("account exists");
        assert_eq!(
            account.original_info.as_ref().map(|info| info.code_hash),
            Some(destroyed.code_hash)
        );
        assert_eq!(account.info.as_ref().map(|info| info.code_hash), Some(KECCAK256_EMPTY));
        assert_eq!(account.status, AccountStatus::DestroyedChanged);

        let evm = create_evm2_from_revm_env(
            EmptyDB::default(),
            RevmSpecId::FRONTIER,
            BlockEnv::default(),
        );
        let mut executor = Evm2TransactionExecutor::new(evm);
        executor.output = output;
        let evm_state = evm_state_from_evm2_with_accounts(&mut executor, resurrection_changes)
            .expect("evm state converts");
        assert!(evm_state.get(&address).expect("account exists").is_created());
        assert_eq!(
            executor
                .current_account_info(address)
                .expect("current account loads")
                .map(|info| info.code_hash),
            Some(KECCAK256_EMPTY)
        );
    }

    #[test]
    fn direct_evm2_executor_applies_pre_and_post_block_system_calls() {
        let block = BlockEnv { number: U256::ONE, ..Default::default() };
        let evm = create_evm2_from_revm_env(EmptyDB::default(), RevmSpecId::PRAGUE, block);
        let mut executor = Evm2TransactionExecutor::new(evm);

        executor
            .apply_pre_block_system_calls(B256::with_last_byte(1), Some(B256::with_last_byte(2)))
            .expect("pre-block system calls execute");
        executor.apply_post_block_system_calls().expect("post-block system calls execute");

        let (_, result) = executor.finish_evm2();
        assert_eq!(result.pre_block_system_results.len(), 2);
        assert_eq!(result.post_block_system_results.len(), 2);
        assert!(result.requests.is_empty());
    }

    #[test]
    fn direct_evm2_executor_applies_beacon_root_system_call() {
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            BEACON_ROOTS_ADDRESS,
            AccountInfo::default().with_code(Bytecode::new_raw(BEACON_ROOTS_CODE.clone())),
        );
        let block = BlockEnv { number: U256::ONE, timestamp: U256::ONE, ..Default::default() };
        let evm = create_evm2_from_revm_env(db, RevmSpecId::CANCUN, block);
        let mut executor = Evm2TransactionExecutor::new(evm);

        executor
            .apply_pre_block_system_calls(B256::ZERO, Some(B256::with_last_byte(0x69)))
            .expect("beacon root system call executes");

        let (state, _) = executor.finish_evm2();
        assert_eq!(
            state
                .account(&BEACON_ROOTS_ADDRESS)
                .and_then(|account| account.storage_slot(U256::ONE)),
            Some(U256::ONE)
        );
        assert_eq!(
            state
                .account(&BEACON_ROOTS_ADDRESS)
                .and_then(|account| account.storage_slot(U256::from(8192))),
            Some(U256::from(0x69))
        );
    }

    #[test]
    fn direct_evm2_executor_execute_block_runs_pre_transactions_and_post() {
        let signer = address!("0x0000000000000000000000000000000000000009");
        let block = BlockEnv { number: U256::ONE, ..Default::default() };
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(signer, AccountInfo::default());
        let evm = create_evm2_from_revm_env(db, RevmSpecId::PRAGUE, block);
        let mut executor = Evm2TransactionExecutor::new(evm);
        let tx = TxLegacy {
            gas_limit: 21_000,
            to: TxKind::Call(address!("0x000000000000000000000000000000000000000a")),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        executor
            .execute_block(
                B256::with_last_byte(1),
                Some(B256::with_last_byte(2)),
                [Recovered::new_unchecked(&tx, signer)],
            )
            .expect("block executes");

        let (state, result) = executor.finish_evm2();
        assert_eq!(result.pre_block_system_results.len(), 2);
        assert_eq!(result.transaction_results.len(), 1);
        assert_eq!(result.post_block_system_results.len(), 2);
        assert_eq!(result.gas_used, 21_000);
        assert!(state.account(&signer).is_some());
    }

    #[test]
    fn direct_evm2_executor_requires_cancun_parent_beacon_block_root() {
        let block = BlockEnv { number: U256::ONE, ..Default::default() };
        let evm = create_evm2_from_revm_env(EmptyDB::default(), RevmSpecId::CANCUN, block);
        let mut executor = Evm2TransactionExecutor::new(evm);

        let error = executor
            .apply_pre_block_system_calls(B256::ZERO, None)
            .expect_err("missing parent beacon block root should fail");

        assert_eq!(error, Evm2BlockExecutionError::MissingParentBeaconBlockRoot);
    }
}
