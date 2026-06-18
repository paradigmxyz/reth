//! Evm2 database adapter for state providers.

use crate::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use alloc::{rc::Rc, vec::Vec};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, BlockNumber, Bytes, B256, U256,
};
use core::{
    cell::RefCell,
    mem,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, CacheDB, Database, Db, DbErrorCode, DynDatabase, StateChangeSource},
    interpreter::Word,
};
use reth_execution_types::{Evm2AccountInfo, Evm2BlockState};
use reth_primitives_traits::{Account, Bytecode as RethBytecode};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie_common::{
    updates::TrieUpdates, AccountProof, ExecutionWitnessMode, HashedPostState,
    HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
    StorageProof, TrieInput,
};
use std::sync::{Arc, Mutex, MutexGuard};

/// An evm2 [`Database`] implementation backed by a Reth [`StateProvider`].
#[derive(Clone)]
pub struct Evm2StateProviderDatabase<DB>(pub DB);

impl<DB> Evm2StateProviderDatabase<DB> {
    /// Creates a new evm2 database adapter.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consumes the adapter and returns the wrapped state provider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB> core::fmt::Debug for Evm2StateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Evm2StateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for Evm2StateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

impl<DB> Deref for Evm2StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB> DerefMut for Evm2StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB> Database for Evm2StateProviderDatabase<DB>
where
    DB: StateProvider + Send + 'static,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(<DB as AccountReader>::basic_account(&self.0, address)?.map(account_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(self.0.evm2_bytecode_by_hash(code_hash)?.unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        <DB as BlockHashReader>::block_hash(&self.0, number)
    }
}

/// An evm2 [`Database`] implementation backed by a borrowed Reth [`StateProvider`].
///
/// evm2 database objects must be `'static` for downcasting. This adapter is only valid while the
/// borrowed provider passed to [`Self::new`] is alive and must not escape the synchronous execution
/// call that created it.
#[derive(Clone, Copy)]
pub struct BorrowedEvm2StateProviderDatabase {
    provider: NonNull<dyn StateProvider>,
}

impl BorrowedEvm2StateProviderDatabase {
    /// Creates a new borrowed evm2 database adapter.
    ///
    /// # Safety
    ///
    /// The returned adapter erases the lifetime of `provider` to satisfy evm2's [`Database`]
    /// downcasting requirements. It must not be used after `provider` is dropped and must not
    /// escape the synchronous execution call that created it.
    pub unsafe fn new(provider: &dyn StateProvider) -> Self {
        let provider = NonNull::from(provider);
        // SAFETY: The caller guarantees the erased lifetime remains valid for every use of the
        // returned adapter.
        let provider = unsafe {
            mem::transmute::<NonNull<dyn StateProvider + '_>, NonNull<dyn StateProvider + 'static>>(
                provider,
            )
        };
        Self { provider }
    }

    fn provider(&self) -> &dyn StateProvider {
        // SAFETY: `provider` is created from a valid shared reference in `new`. Callers must keep
        // that provider alive for the duration of synchronous evm2 execution.
        unsafe { self.provider.as_ref() }
    }
}

impl core::fmt::Debug for BorrowedEvm2StateProviderDatabase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BorrowedEvm2StateProviderDatabase").finish_non_exhaustive()
    }
}

// SAFETY: The adapter only exposes shared `StateProvider` reads and is used for synchronous evm2
// execution. Sending it is sound under the same assumptions as sending the underlying borrowed
// provider reference for read-only access.
unsafe impl Send for BorrowedEvm2StateProviderDatabase {}

impl Database for BorrowedEvm2StateProviderDatabase {
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(AccountReader::basic_account(self.provider(), address)?.map(account_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(self.provider().evm2_bytecode_by_hash(code_hash)?.unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.provider().storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        BlockHashReader::block_hash(self.provider(), number)
    }
}

/// A cloneable evm2 database handle that shares one cache over a borrowed Reth state provider.
///
/// This is intended for sequential execution of multiple short-lived evm2 instances against the
/// same underlying provider. Reads missed by one EVM remain cached for later EVMs, and callers can
/// apply accepted block state with [`Self::commit_source`] to make prior writes visible.
#[derive(Clone)]
pub struct SharedEvm2StateProviderDatabase {
    inner: Arc<Mutex<CacheDB<Db<BorrowedEvm2StateProviderDatabase>>>>,
}

impl SharedEvm2StateProviderDatabase {
    /// Creates a new shared cache over a borrowed state provider.
    ///
    /// # Safety
    ///
    /// This has the same lifetime erasure requirements as
    /// [`BorrowedEvm2StateProviderDatabase::new`]. The returned handle and its clones must not
    /// outlive `provider`.
    pub unsafe fn new(provider: &dyn StateProvider) -> Self {
        Self {
            // SAFETY: The caller upholds the lifetime requirements for the borrowed provider.
            inner: Arc::new(Mutex::new(CacheDB::new(Db::new(unsafe {
                BorrowedEvm2StateProviderDatabase::new(provider)
            })))),
        }
    }

    /// Applies accepted state changes into the shared cache.
    pub fn commit_source<S: StateChangeSource>(&self, source: &S) -> ProviderResult<()> {
        let mut db = self.lock()?;
        db.commit_source(source);
        Ok(())
    }

    fn lock(
        &self,
    ) -> ProviderResult<MutexGuard<'_, CacheDB<Db<BorrowedEvm2StateProviderDatabase>>>> {
        self.inner.lock().map_err(|err| {
            ProviderError::other(std::io::Error::other(format!(
                "shared evm2 database lock poisoned: {err}"
            )))
        })
    }

    fn provider_error(
        db: &mut CacheDB<Db<BorrowedEvm2StateProviderDatabase>>,
        code: DbErrorCode,
    ) -> ProviderError {
        let err = db.error(code);
        match err.downcast::<ProviderError>() {
            Ok(err) => *err,
            Err(err) => ProviderError::other(std::io::Error::other(err.to_string())),
        }
    }
}

impl core::fmt::Debug for SharedEvm2StateProviderDatabase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedEvm2StateProviderDatabase").finish_non_exhaustive()
    }
}

impl Database for SharedEvm2StateProviderDatabase {
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let mut db = self.lock()?;
        db.get_account(address).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        let mut db = self.lock()?;
        db.get_code_by_hash(code_hash).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        let mut db = self.lock()?;
        db.get_storage(address, key).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let mut db = self.lock()?;
        db.get_block_hash(number).map_err(|code| Self::provider_error(&mut db, code))
    }
}

/// A cloneable evm2 database handle that shares one cache over a borrowed Reth state provider
/// within a single thread.
///
/// This avoids synchronization overhead for synchronous replay paths while still preserving cache
/// visibility across short-lived evm2 instances.
#[derive(Clone)]
pub struct LocalEvm2StateProviderDatabase {
    inner: Rc<RefCell<CacheDB<Db<BorrowedEvm2StateProviderDatabase>>>>,
}

impl LocalEvm2StateProviderDatabase {
    /// Creates a new local shared cache over a borrowed state provider.
    ///
    /// # Safety
    ///
    /// This has the same lifetime erasure requirements as
    /// [`BorrowedEvm2StateProviderDatabase::new`]. The returned handle and its clones must not
    /// outlive `provider`.
    pub unsafe fn new(provider: &dyn StateProvider) -> Self {
        Self {
            // SAFETY: The caller upholds the lifetime requirements for the borrowed provider.
            inner: Rc::new(RefCell::new(CacheDB::new(Db::new(unsafe {
                BorrowedEvm2StateProviderDatabase::new(provider)
            })))),
        }
    }

    /// Applies accepted state changes into the local shared cache.
    pub fn commit_source<S: StateChangeSource>(&self, source: &S) {
        self.inner.borrow_mut().commit_source(source);
    }

    fn provider_error(
        db: &mut CacheDB<Db<BorrowedEvm2StateProviderDatabase>>,
        code: DbErrorCode,
    ) -> ProviderError {
        SharedEvm2StateProviderDatabase::provider_error(db, code)
    }
}

impl core::fmt::Debug for LocalEvm2StateProviderDatabase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalEvm2StateProviderDatabase").finish_non_exhaustive()
    }
}

impl Database for LocalEvm2StateProviderDatabase {
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let mut db = self.inner.borrow_mut();
        db.get_account(address).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        let mut db = self.inner.borrow_mut();
        db.get_code_by_hash(code_hash).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        let mut db = self.inner.borrow_mut();
        db.get_storage(address, key).map_err(|code| Self::provider_error(&mut db, code))
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let mut db = self.inner.borrow_mut();
        db.get_block_hash(number).map_err(|code| Self::provider_error(&mut db, code))
    }
}

/// A borrowed [`StateProvider`] that overlays uncommitted evm2 state on top of a base provider.
pub struct Evm2OverlayStateProvider<'a> {
    base: &'a dyn StateProvider,
    overlay: Evm2OverlayStateIndex<'a>,
}

impl core::fmt::Debug for Evm2OverlayStateProvider<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Evm2OverlayStateProvider").finish_non_exhaustive()
    }
}

impl<'a> Evm2OverlayStateProvider<'a> {
    /// Creates a new overlay provider.
    pub fn new(base: &'a dyn StateProvider, overlay: &'a Evm2BlockState) -> Self {
        Self { base, overlay: Evm2OverlayStateIndex::new(overlay) }
    }
}

impl AccountReader for Evm2OverlayStateProvider<'_> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if let Some(account) = self.overlay.accounts.get(address) {
            return Ok(*account)
        }

        self.base.basic_account(address)
    }
}

impl BytecodeReader for Evm2OverlayStateProvider<'_> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<RethBytecode>> {
        if let Some(bytecode) = self.overlay.code.get(code_hash) {
            return Ok(Some(RethBytecode::new_raw(bytecode.original_bytes())))
        }

        self.base.bytecode_by_hash(code_hash)
    }
}

impl BlockHashReader for Evm2OverlayStateProvider<'_> {
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.base.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.base.canonical_hashes_range(start, end)
    }
}

impl StateRootProvider for Evm2OverlayStateProvider<'_> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.base.state_root(hashed_state)
    }

    fn state_root_sorted(&self, hashed_state: HashedPostStateSorted) -> ProviderResult<B256> {
        self.base.state_root_sorted(hashed_state)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.base.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.base.state_root_with_updates(hashed_state)
    }

    fn state_root_sorted_with_updates(
        &self,
        hashed_state: HashedPostStateSorted,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.base.state_root_sorted_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.base.state_root_from_nodes_with_updates(input)
    }
}

impl StorageRootProvider for Evm2OverlayStateProvider<'_> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.base.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.base.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.base.storage_multiproof(address, slots, hashed_storage)
    }
}

impl StateProofProvider for Evm2OverlayStateProvider<'_> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.base.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.base.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        self.base.witness(input, target, mode)
    }
}

impl HashedPostStateProvider for Evm2OverlayStateProvider<'_> {}

impl StateProvider for Evm2OverlayStateProvider<'_> {
    fn evm2_bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(bytecode) = self.overlay.code.get(code_hash) {
            return Ok(Some((*bytecode).clone()))
        }

        self.base.evm2_bytecode_by_hash(code_hash)
    }

    fn storage(&self, account: Address, storage_key: B256) -> ProviderResult<Option<U256>> {
        let slot = U256::from_be_bytes(storage_key.0);
        if let Some(value) =
            self.overlay.storage.get(&account).and_then(|slots| slots.get(&slot)).copied()
        {
            return Ok(Some(value))
        }

        if self.overlay.storage_wipes.contains(&account) {
            return Ok(Some(U256::ZERO))
        }

        self.base.storage(account, storage_key)
    }
}

struct Evm2OverlayStateIndex<'a> {
    accounts: AddressMap<Option<Account>>,
    code: B256Map<&'a Bytecode>,
    storage: AddressMap<U256Map<U256>>,
    storage_wipes: AddressSet,
}

impl<'a> Evm2OverlayStateIndex<'a> {
    fn new(state: &'a Evm2BlockState) -> Self {
        let accounts = state
            .accounts()
            .map(|(address, account)| (address, account.current.as_ref().map(account_info_to_reth)))
            .collect();
        let code = state.code().map(|(hash, bytecode)| (*hash, bytecode)).collect();
        let storage_wipes = state.storage_wipes().collect();
        let mut storage = AddressMap::<U256Map<U256>>::default();
        for (key, slot) in state.storage() {
            storage.entry(key.address()).or_default().insert(key.key(), slot.current);
        }

        Self { accounts, code, storage, storage_wipes }
    }
}

fn account_info_to_reth(info: &Evm2AccountInfo) -> Account {
    let bytecode_hash =
        (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY).then_some(info.code_hash);
    Account { nonce: info.nonce, balance: info.balance, bytecode_hash }
}

fn account_to_evm2(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

fn u256_to_u64_saturating(value: U256) -> BlockNumber {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}
