use alloy_primitives::{
    keccak256,
    map::{AddressMap, AddressSet, B256Set},
    BlockNumber,
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
    prefix_set::{PrefixSetMut, TriePrefixSets, TriePrefixSetsMut},
    Nibbles,
};

/// Load prefix sets using a provider that implements [`ChangeSetReader`]. This function can read
/// changesets from both static files and database.
///
/// Changesets are read one block at a time. Addresses and storage slots are deduplicated across
/// blocks before they are hashed.
pub fn load_prefix_sets_with_provider<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
) -> Result<TriePrefixSets, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + DBProvider,
{
    let mut account_addresses = AddressSet::default();
    let mut storage_slots = AddressMap::<B256Set>::default();
    for block_number in range {
        for AccountBeforeTx { address, .. } in provider.account_block_changeset(block_number)? {
            account_addresses.insert(address);
        }
        for (BlockNumberAddress((_, address)), storage_entry) in
            provider.storage_changeset(block_number)?
        {
            storage_slots.entry(address).or_default().insert(storage_entry.key);
        }
    }

    let mut prefix_sets = TriePrefixSetsMut::default();

    // Sorted hashed addresses make the `HashedAccounts` cursor seek forward only.
    let mut hashed_addresses = account_addresses.into_iter().map(keccak256).collect::<Vec<_>>();
    hashed_addresses.sort_unstable();
    let mut account_hashed_state_cursor =
        provider.tx_ref().cursor_read::<tables::HashedAccounts>()?;
    for hashed_address in hashed_addresses {
        prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));
        if account_hashed_state_cursor.seek_exact(hashed_address)?.is_none() {
            prefix_sets.destroyed_accounts.insert(hashed_address);
        }
    }

    // An address can also be in the account changesets. `PrefixSetMut::freeze` removes the
    // duplicate account prefix.
    for (address, slots) in storage_slots {
        let hashed_address = keccak256(address);
        prefix_sets.account_prefix_set.insert(Nibbles::unpack(hashed_address));
        prefix_sets.storage_prefix_sets.insert(
            hashed_address,
            PrefixSetMut::from(slots.into_iter().map(|slot| Nibbles::unpack(keccak256(slot)))),
        );
    }

    Ok(prefix_sets.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{
        map::{B256Map, HashSet},
        Address, B256, U256,
    };
    use proptest::{collection::btree_set, prelude::*};
    use reth_db_api::{
        models::{StorageBeforeTx, StorageSettings},
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        test_utils::create_test_provider_factory, StaticFileProviderFactory, StaticFileSegment,
        StaticFileWriter,
    };
    use reth_storage_api::StorageSettingsCache;
    use std::collections::BTreeSet;

    /// Changed addresses and `(address, slot)` pairs of one block, as indices.
    type BlockChanges = (BTreeSet<u8>, BTreeSet<(u8, u8)>);

    /// The implementation before changesets were streamed and deduplicated, kept as the reference.
    fn load_prefix_sets_reference<Provider>(
        provider: &Provider,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<TriePrefixSets, ProviderError>
    where
        Provider: ChangeSetReader + StorageChangeSetReader + DBProvider,
    {
        let tx = provider.tx_ref();

        let mut account_prefix_set = PrefixSetMut::default();
        let mut storage_prefix_sets = B256Map::<PrefixSetMut>::default();
        let mut destroyed_accounts = HashSet::default();

        let account_changesets =
            provider.account_changesets_range(*range.start()..*range.end() + 1)?;
        let mut account_hashed_state_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
        for (_, AccountBeforeTx { address, .. }) in account_changesets {
            let hashed_address = keccak256(address);
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            if account_hashed_state_cursor.seek_exact(hashed_address)?.is_none() {
                destroyed_accounts.insert(hashed_address);
            }
        }

        let storage_changesets = provider.storage_changesets_range(range)?;
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

    fn assert_prefix_sets_eq(actual: &TriePrefixSets, expected: &TriePrefixSets) {
        assert_eq!(actual.account_prefix_set.slice(), expected.account_prefix_set.slice());
        assert_eq!(actual.destroyed_accounts, expected.destroyed_accounts);
        assert_eq!(actual.storage_prefix_sets.len(), expected.storage_prefix_sets.len());
        for (hashed_address, expected_set) in &expected.storage_prefix_sets {
            assert_eq!(actual.storage_prefix_sets[hashed_address].slice(), expected_set.slice());
        }
    }

    /// Writes `blocks` as v1 or v2 changesets plus `HashedAccounts` rows for `existing`, then
    /// compares both implementations over the full range and over a range without the first
    /// block.
    fn check_against_reference(blocks: &[BlockChanges], existing: &BTreeSet<u8>, v2: bool) {
        let address = |index: u8| Address::with_last_byte(index);
        let slot = |index: u8| B256::with_last_byte(index);

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(if v2 {
            StorageSettings::v2()
        } else {
            StorageSettings::v1()
        });

        let provider = factory.provider_rw().unwrap();
        for &index in existing {
            provider
                .tx_ref()
                .put::<tables::HashedAccounts>(keccak256(address(index)), Account::default())
                .unwrap();
        }
        if v2 {
            let static_file_provider = factory.static_file_provider();
            let mut account_writer =
                static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            let mut storage_writer =
                static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            for (block_number, (accounts, slots)) in blocks.iter().enumerate() {
                let block_number = block_number as BlockNumber;
                account_writer
                    .append_account_changeset(
                        accounts
                            .iter()
                            .map(|&a| AccountBeforeTx { address: address(a), info: None })
                            .collect(),
                        block_number,
                    )
                    .unwrap();
                storage_writer
                    .append_storage_changeset(
                        slots
                            .iter()
                            .map(|&(a, s)| StorageBeforeTx {
                                address: address(a),
                                key: slot(s),
                                value: U256::ZERO,
                            })
                            .collect(),
                        block_number,
                    )
                    .unwrap();
            }
            account_writer.commit().unwrap();
            storage_writer.commit().unwrap();
        } else {
            for (block_number, (accounts, slots)) in blocks.iter().enumerate() {
                let block_number = block_number as BlockNumber;
                for &a in accounts {
                    provider
                        .tx_ref()
                        .put::<tables::AccountChangeSets>(
                            block_number,
                            AccountBeforeTx { address: address(a), info: None },
                        )
                        .unwrap();
                }
                for &(a, s) in slots {
                    provider
                        .tx_ref()
                        .put::<tables::StorageChangeSets>(
                            BlockNumberAddress((block_number, address(a))),
                            StorageEntry { key: slot(s), value: U256::ZERO },
                        )
                        .unwrap();
                }
            }
        }
        provider.commit().unwrap();

        let provider = factory.provider().unwrap();
        let last_block = blocks.len() as BlockNumber - 1;
        for range in [0..=last_block, 1.min(last_block)..=last_block] {
            let actual = load_prefix_sets_with_provider(&provider, range.clone()).unwrap();
            let expected = load_prefix_sets_reference(&provider, range).unwrap();
            assert_prefix_sets_eq(&actual, &expected);
        }
    }

    fn blocks_strategy() -> impl Strategy<Value = Vec<BlockChanges>> {
        // Few distinct indices, so addresses and slots repeat across blocks.
        prop::collection::vec(
            (btree_set(0u8..12, 0..6), btree_set((0u8..12, 0u8..6), 0..10)),
            1..12,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn matches_reference_v1(blocks in blocks_strategy(), existing in btree_set(0u8..12, 0..12)) {
            check_against_reference(&blocks, &existing, false);
        }

        #[test]
        fn matches_reference_v2(blocks in blocks_strategy(), existing in btree_set(0u8..12, 0..12)) {
            check_against_reference(&blocks, &existing, true);
        }
    }
}
