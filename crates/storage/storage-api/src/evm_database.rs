//! EVM database adapter for state providers.

use crate::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateProviderBox, StateRootProvider, StorageRootProvider,
};
use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, BlockNumber, Bytes, B256, U256,
};
use core::ops::{Deref, DerefMut};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, CacheDB, Database, Db, DynDatabase},
    interpreter::Word,
    ErrorCode,
};
use reth_execution_types::{ExecutionAccountInfo, ExecutionState, ExecutionStateChangeSource};
use reth_primitives_traits::{Account, Bytecode as RethBytecode};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie_common::{
    updates::TrieUpdates, AccountProof, ExecutionWitnessMode, HashedPostState,
    HashedPostStateSorted, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
    StorageProof, TrieInput,
};
use std::sync::{Arc, Mutex, MutexGuard};

/// An EVM [`Database`] implementation backed by a Reth [`StateProvider`].
#[derive(Clone)]
pub struct EvmStateProviderDatabase<DB>(pub DB);

impl<DB> EvmStateProviderDatabase<DB> {
    /// Creates a new EVM database adapter.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consumes the adapter and returns the wrapped state provider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB> core::fmt::Debug for EvmStateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EvmStateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for EvmStateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        self
    }
}

impl<DB> Deref for EvmStateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB> DerefMut for EvmStateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB> Database for EvmStateProviderDatabase<DB>
where
    DB: StateProvider,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(<DB as AccountReader>::basic_account(&self.0, address)?.map(account_to_evm))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(self.0.bytecode_by_hash(code_hash)?.map(Into::into).unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        <DB as BlockHashReader>::block_hash(&self.0, number)
    }
}

/// A cloneable EVM database handle that shares one cache over a Reth state provider.
///
/// This is intended for sequential execution of multiple short-lived EVM instances against the
/// same underlying provider. Reads missed by one EVM remain cached for later EVMs, and callers can
/// apply accepted block state with [`Self::commit_source`] to make prior writes visible.
pub struct SharedEvmStateProviderDatabase<DB: StateProvider = StateProviderBox> {
    inner: Arc<Mutex<CacheDB<Db<EvmStateProviderDatabase<DB>>>>>,
}

impl<DB: StateProvider> Clone for SharedEvmStateProviderDatabase<DB> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<DB> SharedEvmStateProviderDatabase<DB>
where
    DB: StateProvider,
{
    /// Creates a new shared cache over a state provider.
    pub fn new(provider: DB) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheDB::new(Db::new(EvmStateProviderDatabase::new(
                provider,
            ))))),
        }
    }

    /// Applies accepted state changes into the shared cache.
    pub fn commit_source<S: ExecutionStateChangeSource>(&self, source: &S) -> ProviderResult<()> {
        let mut db = self.lock()?;
        db.commit_source(source);
        Ok(())
    }

    fn lock(&self) -> ProviderResult<MutexGuard<'_, CacheDB<Db<EvmStateProviderDatabase<DB>>>>> {
        self.inner.lock().map_err(|err| {
            ProviderError::other(std::io::Error::other(format!(
                "shared EVM database lock poisoned: {err}"
            )))
        })
    }

    fn provider_error(
        db: &mut CacheDB<Db<EvmStateProviderDatabase<DB>>>,
        code: ErrorCode,
    ) -> ProviderError {
        ProviderError::other(std::io::Error::other(db.error(code).to_string()))
    }
}

impl<DB: StateProvider> core::fmt::Debug for SharedEvmStateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedEvmStateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> Database for SharedEvmStateProviderDatabase<DB>
where
    DB: StateProvider,
{
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

/// A borrowed [`StateProvider`] that overlays uncommitted execution state on top of a base
/// provider.
pub struct ExecutionStateOverlayProvider<'a> {
    base: &'a dyn StateProvider,
    overlay: ExecutionStateOverlayIndex<'a>,
}

impl core::fmt::Debug for ExecutionStateOverlayProvider<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExecutionStateOverlayProvider").finish_non_exhaustive()
    }
}

impl<'a> ExecutionStateOverlayProvider<'a> {
    /// Creates a new overlay provider.
    pub fn new(base: &'a dyn StateProvider, overlay: &'a ExecutionState) -> Self {
        Self { base, overlay: ExecutionStateOverlayIndex::new(overlay) }
    }
}

impl AccountReader for ExecutionStateOverlayProvider<'_> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if let Some(account) = self.overlay.accounts.get(address) {
            return Ok(*account)
        }

        self.base.basic_account(address)
    }
}

impl BytecodeReader for ExecutionStateOverlayProvider<'_> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<RethBytecode>> {
        if let Some(bytecode) = self.overlay.code.get(code_hash) {
            return Ok(Some((*bytecode).clone().into()))
        }

        self.base.bytecode_by_hash(code_hash)
    }
}

impl BlockHashReader for ExecutionStateOverlayProvider<'_> {
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

impl StateRootProvider for ExecutionStateOverlayProvider<'_> {
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

impl StorageRootProvider for ExecutionStateOverlayProvider<'_> {
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

impl StateProofProvider for ExecutionStateOverlayProvider<'_> {
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

impl HashedPostStateProvider for ExecutionStateOverlayProvider<'_> {}

impl StateProvider for ExecutionStateOverlayProvider<'_> {
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

struct ExecutionStateOverlayIndex<'a> {
    accounts: AddressMap<Option<Account>>,
    code: B256Map<&'a Bytecode>,
    storage: AddressMap<U256Map<U256>>,
    storage_wipes: AddressSet,
}

impl<'a> ExecutionStateOverlayIndex<'a> {
    fn new(state: &'a ExecutionState) -> Self {
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

fn account_info_to_reth(info: &ExecutionAccountInfo) -> Account {
    let bytecode_hash =
        (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY).then_some(info.code_hash);
    Account { nonce: info.nonce, balance: info.balance, bytecode_hash }
}

fn account_to_evm(account: Account) -> AccountInfo {
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
