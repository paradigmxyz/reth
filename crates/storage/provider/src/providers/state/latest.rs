use crate::{
    providers::{state::macros::delegate_provider_impls, PlainStorageCursor},
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use parking_lot::Mutex;
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{BytecodeReader, DBProvider, StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, KeccakKeyHasher, MultiProof, MultiProofTargets,
    StateRoot, StorageMultiProof, StorageRoot, TrieInput, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot,
    DatabaseTrieWitness,
};

/// State provider over latest state that takes tx reference.
///
/// Wraps a [`DBProvider`] to get access to database.
pub struct LatestStateProviderRef<'b, Provider: DBProvider> {
    /// Database provider.
    provider: &'b Provider,
    /// Optional cached plain storage state cursor (shared with [`LatestStateProvider`]).
    plain_storage_cursor: PlainStorageCursor<Provider::Tx>,
}

impl<Provider: DBProvider + std::fmt::Debug> std::fmt::Debug
    for LatestStateProviderRef<'_, Provider>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatestStateProviderRef").field("provider", &self.provider).finish()
    }
}

impl<'b, Provider: DBProvider> LatestStateProviderRef<'b, Provider> {
    /// Create new state provider
    pub fn new(provider: &'b Provider) -> Self {
        Self { provider, plain_storage_cursor: PlainStorageCursor::<Provider::Tx>::default() }
    }

    /// Create new state provider with a shared cursor.
    pub const fn new_with_cursor(
        provider: &'b Provider,
        plain_storage_cursor: PlainStorageCursor<Provider::Tx>,
    ) -> Self {
        Self { provider, plain_storage_cursor }
    }

    fn plain_storage_cursor(
        &self,
    ) -> ProviderResult<&Mutex<<Provider::Tx as DbTx>::DupCursor<tables::PlainStorageState>>> {
        self.plain_storage_cursor
            .get_or_try_init(|| self.provider.tx_ref().cursor_dup_read().map(Mutex::new))
            .map_err(Into::into)
    }

    fn tx(&self) -> &Provider::Tx {
        self.provider.tx_ref()
    }
}

impl<Provider: DBProvider> AccountReader for LatestStateProviderRef<'_, Provider> {
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.tx().get_by_encoded_key::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<Provider: DBProvider + BlockHashReader> BlockHashReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.provider.canonical_hashes_range(start, end)
    }
}

impl<Provider: DBProvider + Sync> StateRootProvider for LatestStateProviderRef<'_, Provider> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        StateRoot::overlay_root(self.tx(), &hashed_state.into_sorted())
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        StateRoot::overlay_root_from_nodes(self.tx(), TrieInputSorted::from_unsorted(input))
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_with_updates(self.tx(), &hashed_state.into_sorted())
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_from_nodes_with_updates(
            self.tx(),
            TrieInputSorted::from_unsorted(input),
        )
        .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<Provider: DBProvider + Sync> StorageRootProvider for LatestStateProviderRef<'_, Provider> {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        StorageRoot::overlay_root(self.tx(), address, hashed_storage)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        StorageProof::overlay_storage_proof(self.tx(), address, slot, hashed_storage)
            .map_err(ProviderError::from)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        StorageProof::overlay_storage_multiproof(self.tx(), address, slots, hashed_storage)
            .map_err(ProviderError::from)
    }
}

impl<Provider: DBProvider + Sync> StateProofProvider for LatestStateProviderRef<'_, Provider> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let proof = <Proof<_, _> as DatabaseProof>::from_tx(self.tx());
        proof.overlay_account_proof(input, address, slots).map_err(ProviderError::from)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        let proof = <Proof<_, _> as DatabaseProof>::from_tx(self.tx());
        proof.overlay_multiproof(input, targets).map_err(ProviderError::from)
    }

    fn witness(&self, input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        TrieWitness::overlay_witness(self.tx(), input, target)
            .map_err(ProviderError::from)
            .map(|hm| hm.into_values().collect())
    }
}

impl<Provider: DBProvider + Sync> HashedPostStateProvider for LatestStateProviderRef<'_, Provider> {
    fn hashed_post_state(&self, bundle_state: &revm_database::BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<Provider: DBProvider + BlockHashReader> StateProvider
    for LatestStateProviderRef<'_, Provider>
{
    /// Get storage.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        // Use the shared cursor
        if let Some(entry) =
            self.plain_storage_cursor()?.lock().seek_by_key_subkey(account, storage_key)? &&
            entry.key == storage_key
        {
            return Ok(Some(entry.value))
        }
        Ok(None)
    }
}

impl<Provider: DBProvider + BlockHashReader> BytecodeReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.tx().get_by_encoded_key::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

/// State provider for the latest state.
pub struct LatestStateProvider<Provider: DBProvider> {
    /// Database provider.
    provider: Provider,
    /// Cached plain storage state cursor (shared via Arc for ref types).
    plain_storage_cursor: PlainStorageCursor<Provider::Tx>,
}

impl<Provider: DBProvider + std::fmt::Debug> std::fmt::Debug for LatestStateProvider<Provider> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatestStateProvider").field("provider", &self.provider).finish()
    }
}

impl<Provider: DBProvider> LatestStateProvider<Provider> {
    /// Create new state provider
    pub fn new(provider: Provider) -> Self {
        Self { provider, plain_storage_cursor: PlainStorageCursor::<Provider::Tx>::default() }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> LatestStateProviderRef<'_, Provider> {
        LatestStateProviderRef::new_with_cursor(&self.provider, self.plain_storage_cursor.clone())
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<Provider> where [Provider: DBProvider + BlockHashReader ]);

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_state_provider<T: StateProvider>() {}
    #[expect(dead_code)]
    const fn assert_latest_state_provider<T: DBProvider + BlockHashReader>() {
        assert_state_provider::<LatestStateProvider<T>>();
    }
}
