use crate::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProvider, StateRootProvider,
};
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    BytecodeReader, DBProvider, StateProofProvider, StorageRootProvider, StorageSettingsCache,
};
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

    fn hashed_storage_lookup(
        &self,
        hashed_address: B256,
        hashed_slot: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let mut cursor = self.tx().cursor_dup_read::<tables::HashedStorages>()?;
        Ok(cursor
            .seek_by_key_subkey(hashed_address, hashed_slot)?
            .filter(|e| e.key == hashed_slot)
            .map(|e| e.value))
    }
}

impl<Provider: DBProvider + StorageSettingsCache> AccountReader
    for LatestStateProviderRef<'_, Provider>
{
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if self.0.cached_storage_settings().use_hashed_state() {
            let hashed_address = alloy_primitives::keccak256(address);
            self.tx()
                .get_by_encoded_key::<tables::HashedAccounts>(&hashed_address)
                .map_err(Into::into)
        } else {
            self.tx().get_by_encoded_key::<tables::PlainAccountState>(address).map_err(Into::into)
        }
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
        TrieWitness::overlay_witness(self.tx(), input, target).map_err(ProviderError::from).map(
            |hm| {
                let mut v: Vec<_> = hm.into_values().collect();
                v.sort();
                v
            },
        )
    }
}

impl<Provider: DBProvider> HashedPostStateProvider for LatestStateProviderRef<'_, Provider> {
    fn hashed_post_state(&self, bundle_state: &revm_database::BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<Provider: DBProvider + BlockHashReader + StorageSettingsCache> StateProvider
    for LatestStateProviderRef<'_, Provider>
{
    /// Get storage by plain (unhashed) storage key slot.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if self.0.cached_storage_settings().use_hashed_state() {
            self.hashed_storage_lookup(
                alloy_primitives::keccak256(account),
                alloy_primitives::keccak256(storage_key),
            )
        } else {
            let mut cursor = self.tx().cursor_dup_read::<tables::PlainStorageState>()?;
            if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? &&
                entry.key == storage_key
            {
                return Ok(Some(entry.value));
            }
            Ok(None)
        }
    }

    fn storage_by_hashed_key(
        &self,
        address: Address,
        hashed_storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        if self.0.cached_storage_settings().use_hashed_state() {
            self.hashed_storage_lookup(alloy_primitives::keccak256(address), hashed_storage_key)
        } else {
            Err(ProviderError::UnsupportedProvider)
        }
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
pub struct LatestStateProvider<Provider>(Provider);

impl<Provider: DBProvider> LatestStateProvider<Provider> {
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

// Delegates all provider impls to [LatestStateProviderRef]
reth_storage_api::macros::delegate_provider_impls!(LatestStateProvider<Provider> where [Provider: DBProvider + BlockHashReader + StorageSettingsCache]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_provider_factory;
    use alloy_primitives::{address, b256, keccak256, U256};
    use reth_db_api::{
        models::StorageSettings,
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::StorageEntry;
    use reth_storage_api::StorageSettingsCache;
    use reth_storage_errors::provider::ProviderError;

    const fn assert_state_provider<T: StateProvider>() {}
    #[expect(dead_code)]
    const fn assert_latest_state_provider<
        T: DBProvider + BlockHashReader + StorageSettingsCache,
    >() {
        assert_state_provider::<LatestStateProvider<T>>();
    }

    #[test]
    fn test_latest_storage_hashed_state() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: hashed_slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert_eq!(provider_ref.storage(address, slot).unwrap(), Some(U256::from(42)));

        let other_address = address!("0x0000000000000000000000000000000000000099");
        let other_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000099");
        assert_eq!(provider_ref.storage(other_address, other_slot).unwrap(), None);

        let tx = factory.provider_rw().unwrap().into_tx();
        let plain_address = address!("0x0000000000000000000000000000000000000002");
        let plain_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000002");
        tx.put::<tables::PlainStorageState>(
            plain_address,
            StorageEntry { key: plain_slot, value: U256::from(99) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(plain_address, plain_slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_hashed_state_returns_none_for_missing() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(address, slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_legacy() {
        let factory = create_test_provider_factory();
        assert!(!factory.provider().unwrap().cached_storage_settings().use_hashed_state());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000005");

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::PlainStorageState>(
            address,
            StorageEntry { key: slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert_eq!(provider_ref.storage(address, slot).unwrap(), Some(U256::from(42)));

        let other_slot =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000099");
        assert_eq!(provider_ref.storage(address, other_slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_legacy_does_not_read_hashed() {
        let factory = create_test_provider_factory();
        assert!(!factory.provider().unwrap().cached_storage_settings().use_hashed_state());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000005");
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: hashed_slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);
        assert_eq!(provider_ref.storage(address, slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_by_hashed_key_v2() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);

        let tx = factory.provider_rw().unwrap().into_tx();
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: hashed_slot, value: U256::from(42) },
        )
        .unwrap();
        tx.commit().unwrap();

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert_eq!(
            provider_ref.storage_by_hashed_key(address, hashed_slot).unwrap(),
            Some(U256::from(42))
        );

        assert_eq!(provider_ref.storage_by_hashed_key(address, slot).unwrap(), None);
    }

    #[test]
    fn test_latest_storage_by_hashed_key_unsupported_in_v1() {
        let factory = create_test_provider_factory();
        assert!(!factory.provider().unwrap().cached_storage_settings().use_hashed_state());

        let address = address!("0x0000000000000000000000000000000000000001");
        let slot = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

        let db = factory.provider().unwrap();
        let provider_ref = LatestStateProviderRef::new(&db);

        assert!(matches!(
            provider_ref.storage_by_hashed_key(address, slot),
            Err(ProviderError::UnsupportedProvider)
        ));
    }
}
