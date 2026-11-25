use alloy_primitives::{
    keccak256,
    map::{B256Map, HashSet},
    Address, BlockNumber, B256,
};
use core::ops::RangeInclusive;
use reth_db_api::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::DbTx,
};
use reth_storage_api::{ChangeSetReader, DBProvider, StorageChangeSetReader};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    Nibbles,
};

/// Load prefix sets using a provider that implements [`ChangeSetReader`]. This function can read
/// changesets from both static files and database.
pub fn load_prefix_sets_with_provider<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
) -> Result<TriePrefixSets, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + DBProvider,
{
    let tx = provider.tx_ref();

    // Initialize prefix sets.
    let mut account_prefix_set = PrefixSetMut::default();
    let mut storage_prefix_sets = B256Map::<PrefixSetMut>::default();
    let mut destroyed_accounts = HashSet::default();

    // Get account changesets using the provider (handles static files + database)
    let account_changesets = provider.account_changesets_range(*range.start()..*range.end() + 1)?;

    // We still need direct access to HashedAccounts table
    let mut account_hashed_state_cursor = tx.cursor_read::<tables::HashedAccounts>()?;

    for (_, AccountBeforeTx { address, .. }) in account_changesets {
        let hashed_address = keccak256(address);
        account_prefix_set.insert(Nibbles::unpack(hashed_address));

        if account_hashed_state_cursor.seek_exact(hashed_address)?.is_none() {
            destroyed_accounts.insert(hashed_address);
        }
    }

    // Storage wipe changesets are keyed by hashed slot.
    for (BlockNumberAddress((_, address)), storage_entry) in
        provider.storage_wipe_changesets_range(range.clone())?
    {
        let hashed_address = keccak256(address);
        account_prefix_set.insert(Nibbles::unpack(hashed_address));
        storage_prefix_sets
            .entry(hashed_address)
            .or_default()
            .insert(Nibbles::unpack(storage_entry.key));
    }

    // Walk storage changesets using the provider (handles static files + database)
    let storage_changesets = provider.storage_changesets_range(range)?;
    // Rows are ordered by `BlockNumberAddress`, so all slots of one account arrive consecutively.
    // Memoizing the last address avoids re-hashing it once per slot; the repeated account prefix
    // insert can be skipped because `PrefixSetMut::freeze` deduplicates anyway.
    let mut last_address: Option<(Address, B256)> = None;
    for (BlockNumberAddress((_, address)), storage_entry) in storage_changesets {
        let hashed_address = match last_address {
            Some((last, hashed)) if last == address => hashed,
            _ => {
                let hashed = keccak256(address);
                account_prefix_set.insert(Nibbles::unpack(hashed));
                last_address = Some((address, hashed));
                hashed
            }
        };
        storage_prefix_sets
            .entry(hashed_address)
            .or_default()
            .insert(Nibbles::unpack(keccak256(storage_entry.key)));
    }

    Ok(TriePrefixSets {
        account_prefix_set: account_prefix_set.freeze(),
        storage_prefix_sets: storage_prefix_sets
            .into_iter()
            .map(|(k, v)| (k, v.freeze()))
            .collect(),
        destroyed_accounts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::U256;
    use reth_db_api::{models::StorageSettings, transaction::DbTxMut};
    use reth_primitives_traits::StorageEntry;
    use reth_provider::{
        test_utils::create_test_provider_factory, StaticFileProviderFactory, StaticFileSegment,
        StaticFileWriter, StorageSettingsCache,
    };

    #[test]
    fn load_prefix_sets_includes_storage_wipe_changesets() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        {
            let sf = factory.static_file_provider();
            let mut headers = sf.latest_writer(StaticFileSegment::Headers).unwrap();
            for number in 0..=1 {
                let header = Header { number, ..Default::default() };
                headers.append_header(&header, &B256::ZERO).unwrap();
            }
            headers.commit().unwrap();
            let mut accounts = sf.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            let mut storages = sf.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            for block in 0..=1 {
                accounts.append_account_changeset(vec![], block).unwrap();
                storages.append_storage_changeset(vec![], block).unwrap();
            }
            accounts.commit().unwrap();
            storages.commit().unwrap();
        }

        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(B256::with_last_byte(1));
        let provider = factory.provider_rw().unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageWipeChangeSets>(
                BlockNumberAddress((1, address)),
                StorageEntry { key: hashed_slot, value: U256::from(9) },
            )
            .unwrap();

        let mut prefix_sets = load_prefix_sets_with_provider(&*provider, 1..=1).unwrap();

        assert!(prefix_sets.account_prefix_set.contains(&Nibbles::unpack(hashed_address)));
        assert!(prefix_sets
            .storage_prefix_sets
            .get_mut(&hashed_address)
            .is_some_and(|prefix_set| prefix_set.contains(&Nibbles::unpack(hashed_slot))));
    }
}
