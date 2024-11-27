use crate::{
    providers::state::macros::delegate_provider_impls, AccountReader, BlockHashReader,
    HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, BlockNumber, Bytes, StorageKey, StorageValue, B256,
};
use reth_db::tables;
use reth_db_api::{cursor::DbDupCursorRO, transaction::DbTx};
use reth_primitives::{Account, Bytecode};
use reth_storage_api::{
    DBProvider, StateCommitmentProvider, StateProofProvider, StorageRootProvider,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, MultiProof, StateRoot, StorageMultiProof,
    StorageRoot, TrieInput,
};
use reth_trie_db::{
    DatabaseProof, DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot,
    DatabaseTrieWitness, StateCommitment,
};

/// State provider over latest state that takes tx reference.
///
/// Wraps a [`DBProvider`] to get access to database.
#[derive(Debug)]
pub struct LatestStateProviderRef<'b, Provider>(&'b Provider);

impl<'b, Provider: DBProvider> LatestStateProviderRef<'b, Provider> {
    /// Create new state provider
    pub const fn new(provider: &'b Provider) -> Self {
        Self(provider)
    }

    fn tx(&self) -> &Provider::Tx {
        self.0.tx_ref()
    }
}

impl<Provider: DBProvider> AccountReader for LatestStateProviderRef<'_, Provider> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.tx().get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<Provider: BlockHashReader> BlockHashReader for LatestStateProviderRef<'_, Provider> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.0.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.0.canonical_hashes_range(start, end)
    }
}

impl<Provider: DBProvider + StateCommitmentProvider> StateRootProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        StateRoot::overlay_root(self.tx(), hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        StateRoot::overlay_root_from_nodes(self.tx(), input)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_with_updates(self.tx(), hashed_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        StateRoot::overlay_root_from_nodes_with_updates(self.tx(), input)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<Provider: DBProvider + StateCommitmentProvider> StorageRootProvider
    for LatestStateProviderRef<'_, Provider>
{
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
            .map_err(Into::<ProviderError>::into)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        StorageProof::overlay_storage_multiproof(self.tx(), address, slots, hashed_storage)
            .map_err(Into::<ProviderError>::into)
    }
}

impl<Provider: DBProvider + StateCommitmentProvider> StateProofProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Proof::overlay_account_proof(self.tx(), input, address, slots)
            .map_err(Into::<ProviderError>::into)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        Proof::overlay_multiproof(self.tx(), input, targets).map_err(Into::<ProviderError>::into)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        TrieWitness::overlay_witness(self.tx(), input, target).map_err(Into::<ProviderError>::into)
    }
}

impl<Provider: DBProvider + StateCommitmentProvider> HashedPostStateProvider
    for LatestStateProviderRef<'_, Provider>
{
    fn hashed_post_state(&self, bundle_state: &revm::db::BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<
            <Provider::StateCommitment as StateCommitment>::KeyHasher,
        >(bundle_state.state())
    }
}

impl<Provider: DBProvider + BlockHashReader + StateCommitmentProvider> StateProvider
    for LatestStateProviderRef<'_, Provider>
{
    /// Get storage.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let mut cursor = self.tx().cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        self.tx().get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

impl<Provider: StateCommitmentProvider> StateCommitmentProvider
    for LatestStateProviderRef<'_, Provider>
{
    type StateCommitment = Provider::StateCommitment;
}

/// State provider for the latest state.
#[derive(Debug)]
pub struct LatestStateProvider<Provider>(Provider);

impl<Provider: DBProvider + StateCommitmentProvider> LatestStateProvider<Provider> {
    /// Create new state provider
    pub const fn new(db: Provider) -> Self {
        Self(db)
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    const fn as_ref(&self) -> LatestStateProviderRef<'_, Provider> {
        LatestStateProviderRef::new(&self.0)
    }
}

impl<Provider: StateCommitmentProvider> StateCommitmentProvider for LatestStateProvider<Provider> {
    type StateCommitment = Provider::StateCommitment;
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<Provider> where [Provider: DBProvider + BlockHashReader + StateCommitmentProvider]);

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_state_provider<T: StateProvider>() {}
    #[allow(dead_code)]
    const fn assert_latest_state_provider<
        T: DBProvider + BlockHashReader + StateCommitmentProvider,
    >() {
        assert_state_provider::<LatestStateProvider<T>>();
    }
}
