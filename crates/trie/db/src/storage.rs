use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, TrieTableAdapter};
use alloy_primitives::{keccak256, map::hash_map, Address, BlockNumber, B256};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StorageRootError;
use reth_storage_api::{BlockNumReader, StorageChangeSetReader};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    HashedStorage, StorageRoot, TrieInputSorted,
};

#[cfg(feature = "metrics")]
use reth_trie::metrics::TrieRootMetrics;

/// Extends [`StorageRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStorageRoot<'a, TX> {
    /// Create a new storage root calculator from database transaction and raw address.
    fn from_tx(tx: &'a TX, address: Address) -> Self;

    /// Create a new storage root calculator from database transaction and hashed address.
    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self;

    /// Calculates the storage root with the given trie input.
    fn overlay_root(
        tx: &'a TX,
        address: Address,
        input: TrieInputSorted,
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
    let mut storage = HashedStorage::default();
    let tip = provider.last_block_number()?;

    if from > tip {
        return Ok(storage)
    }

    // Storage wipe changesets are keyed by hashed slot. A stable sort by block keeps the oldest
    // value of each slot first.
    let mut changesets = provider
        .storage_changesets_range(from..=tip)?
        .into_iter()
        .map(|(key, entry)| (key, keccak256(entry.key), entry.value))
        .chain(
            provider
                .storage_wipe_changesets_range(from..=tip)?
                .into_iter()
                .map(|(key, entry)| (key, entry.key, entry.value)),
        )
        .filter(|(key, _, _)| key.address() == address)
        .collect::<Vec<_>>();
    changesets.sort_by_key(|(key, _, _)| key.block_number());
    for (_, hashed_slot, value) in changesets {
        if let hash_map::Entry::Vacant(entry) = storage.storage.entry(hashed_slot) {
            entry.insert(value);
        }
    }

    Ok(storage)
}

impl<'a, TX: DbTx, A: TrieTableAdapter> DatabaseStorageRoot<'a, TX>
    for StorageRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX, address: Address) -> Self {
        Self::new(
            DatabaseTrieCursorFactory::<_, A>::new(tx),
            DatabaseHashedCursorFactory::new(tx),
            address,
            Default::default(),
            #[cfg(feature = "metrics")]
            TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
    }

    fn from_tx_hashed(tx: &'a TX, hashed_address: B256) -> Self {
        Self::new_hashed(
            DatabaseTrieCursorFactory::<_, A>::new(tx),
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
        mut input: TrieInputSorted,
    ) -> Result<B256, StorageRootError> {
        let hashed_address = keccak256(address);
        StorageRoot::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::<_, A>::new(tx),
                &input.nodes,
            ),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &input.state),
            address,
            input
                .prefix_sets
                .storage_prefix_sets
                .remove(&hashed_address)
                .unwrap_or_default()
                .freeze(),
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
    use reth_db_api::{
        models::{BlockNumberAddress, StorageBeforeTx, StorageSettings},
        tables,
        transaction::DbTxMut,
    };
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
        let slot1 = B256::with_last_byte(100);
        let slot2 = B256::with_last_byte(200);

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
        let factory = create_test_provider_factory();

        factory.set_storage_settings_cache(StorageSettings::v2());

        let provider = factory.provider_rw().unwrap();
        assert!(provider.cached_storage_settings().use_hashed_state());
        assert!(provider.cached_storage_settings().is_v2());

        let address = Address::with_last_byte(42);
        let plain_slot1 = B256::with_last_byte(100);
        let plain_slot2 = B256::with_last_byte(200);
        let hashed_slot1 = keccak256(plain_slot1);
        let hashed_slot2 = keccak256(plain_slot2);

        append_headers_to_static_files(&factory, 5);

        append_storage_changesets_to_static_files(
            &factory,
            vec![
                (0, vec![]),
                (1, vec![StorageBeforeTx { address, key: plain_slot1, value: U256::from(10) }]),
                (2, vec![StorageBeforeTx { address, key: plain_slot2, value: U256::from(20) }]),
                (3, vec![StorageBeforeTx { address, key: plain_slot1, value: U256::from(999) }]),
            ],
        );

        let result = hashed_storage_from_reverts_with_provider(&*provider, address, 1).unwrap();

        assert_eq!(result.storage.len(), 2);
        assert_eq!(result.storage.get(&hashed_slot1), Some(&U256::from(10)));
        assert_eq!(result.storage.get(&hashed_slot2), Some(&U256::from(20)));
    }

    #[test]
    fn hashed_storage_from_reverts_includes_storage_wipe_changesets() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let provider = factory.provider_rw().unwrap();

        let address = Address::with_last_byte(42);
        let [slot1, slot2, slot3] = [1, 2, 3].map(B256::with_last_byte);
        let [hashed_slot1, hashed_slot2, hashed_slot3] = [slot1, slot2, slot3].map(keccak256);

        append_headers_to_static_files(&factory, 5);
        append_storage_changesets_to_static_files(
            &factory,
            vec![
                (0, vec![]),
                (1, vec![StorageBeforeTx { address, key: slot1, value: U256::from(10) }]),
                (2, vec![]),
                (3, vec![StorageBeforeTx { address, key: slot2, value: U256::from(999) }]),
            ],
        );
        // Block 2 wipes the storage. Its row for `slot1` is newer than the changeset of block 1.
        for (key, value) in [(hashed_slot1, 77), (hashed_slot2, 20), (hashed_slot3, 30)] {
            provider
                .tx_ref()
                .put::<tables::StorageWipeChangeSets>(
                    BlockNumberAddress((2, address)),
                    StorageEntry { key, value: U256::from(value) },
                )
                .unwrap();
        }

        let result = hashed_storage_from_reverts_with_provider(&*provider, address, 1).unwrap();

        assert_eq!(
            result.storage,
            [(hashed_slot1, 10), (hashed_slot2, 20), (hashed_slot3, 30)]
                .map(|(slot, value)| (slot, U256::from(value)))
                .into_iter()
                .collect()
        );
    }
}
