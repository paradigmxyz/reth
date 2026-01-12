use crate::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{BytecodeReader, DBProvider, StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::{ProviderError, ProviderResult};

#[cfg(feature = "triedb")]
use crate::providers::TrieDBProvider;
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
#[derive(Debug)]
pub struct LatestStateProviderRef<'b, Provider> {
    provider: &'b Provider,
    #[cfg(feature = "triedb")]
    triedb_provider: Option<&'b TrieDBProvider>,
}

impl<'b, Provider: DBProvider> LatestStateProviderRef<'b, Provider> {
    /// Create new state provider
    pub const fn new(provider: &'b Provider) -> Self {
        Self {
            provider,
            #[cfg(feature = "triedb")]
            triedb_provider: None,
        }
    }

    /// Create new state provider with TrieDB support
    #[cfg(feature = "triedb")]
    pub const fn new_with_triedb(
        provider: &'b Provider,
        triedb_provider: &'b TrieDBProvider,
    ) -> Self {
        Self { provider, triedb_provider: Some(triedb_provider) }
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

impl<Provider: BlockHashReader> BlockHashReader for LatestStateProviderRef<'_, Provider> {
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

impl<Provider: DBProvider> StateRootProvider for LatestStateProviderRef<'_, Provider> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        Ok(StateRoot::overlay_root(self.tx(), &hashed_state.into_sorted())?)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        Ok(StateRoot::overlay_root_from_nodes(self.tx(), TrieInputSorted::from_unsorted(input))?)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok(StateRoot::overlay_root_with_updates(self.tx(), &hashed_state.into_sorted())?)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok(StateRoot::overlay_root_from_nodes_with_updates(
            self.tx(),
            TrieInputSorted::from_unsorted(input),
        )?)
    }

    #[cfg(feature = "triedb")]
    fn state_root_with_updates_plain(
        &self,
        plain_state: reth_storage_api::PlainPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        // Use TrieDB if available, otherwise return error
        if let Some(triedb_provider) = self.triedb_provider {
            tracing::debug!(target: "reth_provider", "Using TrieDB for state root computation");
            crate::providers::state_root_with_updates_triedb(triedb_provider, plain_state)
        } else {
            tracing::error!(target: "reth_provider", "TrieDB provider not available, returning UnsupportedProvider error");
            Err(ProviderError::UnsupportedProvider)
        }
    }
}

impl<Provider: DBProvider> StorageRootProvider for LatestStateProviderRef<'_, Provider> {
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

impl<Provider: DBProvider> StateProofProvider for LatestStateProviderRef<'_, Provider> {
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

impl<Provider: DBProvider> HashedPostStateProvider for LatestStateProviderRef<'_, Provider> {
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
        let mut cursor = self.tx().cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? &&
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
#[derive(Debug)]
pub struct LatestStateProvider<Provider> {
    provider: Provider,
    #[cfg(feature = "triedb")]
    triedb_provider: Option<TrieDBProvider>,
}

impl<Provider: DBProvider> LatestStateProvider<Provider> {
    /// Create new state provider
    pub const fn new(provider: Provider) -> Self {
        Self {
            provider,
            #[cfg(feature = "triedb")]
            triedb_provider: None,
        }
    }

    /// Create new state provider with TrieDB support
    #[cfg(feature = "triedb")]
    pub fn new_with_triedb(provider: Provider, triedb_provider: TrieDBProvider) -> Self {
        tracing::debug!(target: "reth_provider", "Creating LatestStateProvider with TrieDB support");
        Self { provider, triedb_provider: Some(triedb_provider) }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> LatestStateProviderRef<'_, Provider> {
        #[cfg(feature = "triedb")]
        if let Some(ref triedb) = self.triedb_provider {
            return LatestStateProviderRef::new_with_triedb(&self.provider, triedb);
        }

        LatestStateProviderRef::new(&self.provider)
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
reth_storage_api::macros::delegate_provider_impls!(LatestStateProvider<Provider> where [Provider: DBProvider + BlockHashReader ]);

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_state_provider<T: StateProvider>() {}
    #[expect(dead_code)]
    const fn assert_latest_state_provider<T: DBProvider + BlockHashReader>() {
        assert_state_provider::<LatestStateProvider<T>>();
    }
}
