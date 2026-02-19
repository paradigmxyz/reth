use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{keccak256, map::hash_map, Address, BlockNumber, B256};
use reth_db_api::{models::BlockNumberAddress, transaction::DbTx};
use reth_execution_errors::StorageRootError;
use reth_storage_api::{BlockNumReader, StorageChangeSetReader};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, HashedPostState, HashedStorage, StorageRoot,
};

#[cfg(feature = "metrics")]
use reth_trie::metrics::TrieRootMetrics;

/// Extends [`StorageRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStorageRoot<'a, TX> {
    /// Create a new storage root calculator from database transaction and raw address.
    fn from_tx(tx: &'a TX, address: Address) -> Self;

    /// Create a new storage root calculator from database transaction and hashed address.
    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self;

    /// Calculates the storage root for this [`HashedStorage`] and returns it.
    fn overlay_root(
        tx: &'a TX,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError>;
}

/// Initializes [`HashedStorage`] from reverts using a provider.
pub fn hashed_storage_from_reverts_with_provider<P>(
    provider: &P,
    address: Address,
    from: BlockNumber,
) -> ProviderResult<HashedStorage>
where
    P: StorageChangeSetReader + BlockNumReader,
{
    let mut storage = HashedStorage::new(false);
    let tip = provider.last_block_number()?;

    if from > tip {
        return Ok(storage)
    }

    for (BlockNumberAddress((_, storage_address)), storage_change) in
        provider.storage_changesets_range(from..=tip)?
    {
        if storage_address == address {
            let hashed_slot = keccak256(storage_change.key);
            if let hash_map::Entry::Vacant(entry) = storage.storage.entry(hashed_slot) {
                entry.insert(storage_change.value);
            }
        }
    }

    Ok(storage)
}

impl<'a, TX: DbTx> DatabaseStorageRoot<'a, TX>
    for StorageRoot<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self {
        Self::new_hashed(
            DatabaseTrieCursorFactory::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            hashed_address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn overlay_root(
        tx: &'a TX,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> Result<B256, StorageRootError> {
        let prefix_set = hashed_storage.construct_prefix_set().freeze();
        let state_sorted =
            HashedPostState::from_hashed_storage(keccak256(address), hashed_storage).into_sorted();
        StorageRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
            address,
            prefix_set,
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
        .root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::U256;
    use reth_db_api::{models::BlockNumberAddress, tables, transaction::DbTxMut};
    use reth_primitives_traits::StorageEntry;
    use reth_provider::{
        test_utils::create_test_provider_factory, StaticFileProviderFactory, StaticFileSegment,
        StaticFileWriter, StorageSettingsCache,
    };

    fn append_storage_changesets_to_static_files(
        factory: &impl StaticFileProviderFactory<
            Primitives: reth_primitives_traits::NodePrimitives<BlockHeader = Header>,
        >,
        changesets: Vec<(u64, Vec<reth_db_api::models::StorageBeforeTx>)>,
    ) {
        let sf = factory.static_file_provider();
        let mut writer = sf.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
        for (block_number, changeset) in changesets {
            writer.append_storage_changeset(changeset, block_number).unwrap();
        }
        writer.commit().unwrap();
    }

    fn append_headers_to_static_files(
        factory: &impl StaticFileProviderFactory<
            Primitives: reth_primitives_traits::NodePrimitives<BlockHeader = Header>,
        >,
        up_to_block: u64,
    ) {
        let sf = factory.static_file_provider();
        let mut writer = sf.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut header = Header::default();
        for num in 0..=up_to_block {
            header.number = num;
            writer.append_header(&header, &B256::ZERO).unwrap();
        }
        writer.commit().unwrap();
    }

    #[test]
    fn test_hashed_storage_from_reverts_legacy() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        assert!(!provider.cached_storage_settings().use_hashed_state());

        let address = Address::with_last_byte(42);
        let slot1 = B256::from(U256::from(100));
        let slot2 = B256::from(U256::from(200));

        append_headers_to_static_files(&factory, 5);

        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((1, address)),
                StorageEntry { key: slot1, value: U256::from(10) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address)),
                StorageEntry { key: slot2, value: U256::from(20) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address)),
                StorageEntry { key: slot1, value: U256::from(999) },
            )
            .unwrap();

        let result = hashed_storage_from_reverts_with_provider(&*provider, address, 1).unwrap();

        let hashed_slot1 = keccak256(slot1);
        let hashed_slot2 = keccak256(slot2);

        assert_eq!(result.storage.len(), 2);
        assert_eq!(result.storage.get(&hashed_slot1), Some(&U256::from(10)));
        assert_eq!(result.storage.get(&hashed_slot2), Some(&U256::from(20)));
    }

    #[test]
    fn test_hashed_storage_from_reverts_hashed_state() {
        use reth_db_api::models::{StorageBeforeTx, StorageSettings};

        let factory = create_test_provider_factory();

        factory.set_storage_settings_cache(StorageSettings::v2());

        let provider = factory.provider_rw().unwrap();
        assert!(provider.cached_storage_settings().use_hashed_state());
        assert!(provider.cached_storage_settings().is_v2());

        let address = Address::with_last_byte(42);
        let plain_slot1 = B256::from(U256::from(100));
        let plain_slot2 = B256::from(U256::from(200));
        let hashed_slot1 = keccak256(plain_slot1);
        let hashed_slot2 = keccak256(plain_slot2);

        append_headers_to_static_files(&factory, 5);

        append_storage_changesets_to_static_files(
            &factory,
            vec![
                (0, vec![]),
                (1, vec![StorageBeforeTx { address, key: hashed_slot1, value: U256::from(10) }]),
                (2, vec![StorageBeforeTx { address, key: hashed_slot2, value: U256::from(20) }]),
                (3, vec![StorageBeforeTx { address, key: hashed_slot1, value: U256::from(999) }]),
            ],
        );

        let result = hashed_storage_from_reverts_with_provider(&*provider, address, 1).unwrap();

        assert_eq!(result.storage.len(), 2);
        assert_eq!(result.storage.get(&hashed_slot1), Some(&U256::from(10)));
        assert_eq!(result.storage.get(&hashed_slot2), Some(&U256::from(20)));
    }
}
