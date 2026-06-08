//! Evm2 database adapter for state providers.

use crate::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Address, BlockNumber, Bytes, B256, U256};
use core::{
    mem,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
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

/// A borrowed [`StateProvider`] that overlays uncommitted evm2 state on top of a base provider.
pub struct Evm2OverlayStateProvider<'a> {
    base: &'a dyn StateProvider,
    overlay: &'a Evm2BlockState,
}

impl core::fmt::Debug for Evm2OverlayStateProvider<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Evm2OverlayStateProvider").finish_non_exhaustive()
    }
}

impl<'a> Evm2OverlayStateProvider<'a> {
    /// Creates a new overlay provider.
    pub const fn new(base: &'a dyn StateProvider, overlay: &'a Evm2BlockState) -> Self {
        Self { base, overlay }
    }
}

impl AccountReader for Evm2OverlayStateProvider<'_> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if let Some((_, account)) = self.overlay.accounts().find(|(addr, _)| addr == address) {
            return Ok(account.current.as_ref().map(account_info_to_reth))
        }

        self.base.basic_account(address)
    }
}

impl BytecodeReader for Evm2OverlayStateProvider<'_> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<RethBytecode>> {
        if let Some((_, bytecode)) = self.overlay.code().find(|(hash, _)| *hash == code_hash) {
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
        if let Some((_, bytecode)) = self.overlay.code().find(|(hash, _)| *hash == code_hash) {
            return Ok(Some(bytecode.clone()))
        }

        self.base.evm2_bytecode_by_hash(code_hash)
    }

    fn storage(&self, account: Address, storage_key: B256) -> ProviderResult<Option<U256>> {
        let slot = U256::from_be_bytes(storage_key.0);
        if let Some((_, storage)) =
            self.overlay.storage().find(|(key, _)| key.address() == account && key.key() == slot)
        {
            return Ok(Some(storage.current))
        }

        if self.overlay.storage_wipes().any(|address| address == account) {
            return Ok(Some(U256::ZERO))
        }

        self.base.storage(account, storage_key)
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
