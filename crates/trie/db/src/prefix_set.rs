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
/// The provider streams the changesets of the range. Addresses and storage slots are deduplicated
/// across blocks before they are hashed.
pub fn load_prefix_sets_with_provider<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
) -> Result<TriePrefixSets, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + DBProvider,
{
    let mut account_addresses = AddressSet::default();
    provider.for_each_account_changeset(range.clone(), |_, AccountBeforeTx { address, .. }| {
        account_addresses.insert(address);
    })?;

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

    // Storage is scanned after the account loop, so the hashed addresses are already dropped.
    let mut storage_slots = AddressMap::<B256Set>::default();
    provider.for_each_storage_changeset(range, |BlockNumberAddress((_, address)), entry| {
        storage_slots.entry(address).or_default().insert(entry.key);
    })?;

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
    use alloy_primitives::{Address, B256, U256};
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

    /// Changed addresses of each block, as indices.
    type AccountBlocks = Vec<BTreeSet<u8>>;

    /// Changed `(address, slot)` pairs of each block, as indices.
    type StorageBlocks = Vec<BTreeSet<(u8, u8)>>;

    fn assert_prefix_sets_eq(actual: &TriePrefixSets, expected: &TriePrefixSets) {
        assert_eq!(actual.account_prefix_set.slice(), expected.account_prefix_set.slice());
        assert_eq!(actual.destroyed_accounts, expected.destroyed_accounts);
        assert_eq!(actual.storage_prefix_sets.len(), expected.storage_prefix_sets.len());
        for (hashed_address, expected_set) in &expected.storage_prefix_sets {
            assert_eq!(actual.storage_prefix_sets[hashed_address].slice(), expected_set.slice());
        }
    }

    /// Writes the blocks as v1 or v2 changesets plus `HashedAccounts` rows for `existing`, then
    /// compares the loaded prefix sets with sets built straight from the fixture over the full
    /// range, over a range without the first block, and over a range that ends past both tips.
    /// The two tables can end at different blocks, so on v2 the segments have different tips.
    fn check_against_fixture(
        account_blocks: &AccountBlocks,
        storage_blocks: &StorageBlocks,
        existing: &BTreeSet<u8>,
        v2: bool,
    ) {
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
            for (block_number, accounts) in account_blocks.iter().enumerate() {
                account_writer
                    .append_account_changeset(
                        accounts
                            .iter()
                            .map(|&a| AccountBeforeTx { address: address(a), info: None })
                            .collect(),
                        block_number as BlockNumber,
                    )
                    .unwrap();
            }
            for (block_number, slots) in storage_blocks.iter().enumerate() {
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
                        block_number as BlockNumber,
                    )
                    .unwrap();
            }
            account_writer.commit().unwrap();
            storage_writer.commit().unwrap();
        } else {
            for (block_number, accounts) in account_blocks.iter().enumerate() {
                for &a in accounts {
                    provider
                        .tx_ref()
                        .put::<tables::AccountChangeSets>(
                            block_number as BlockNumber,
                            AccountBeforeTx { address: address(a), info: None },
                        )
                        .unwrap();
                }
            }
            for (block_number, slots) in storage_blocks.iter().enumerate() {
                for &(a, s) in slots {
                    provider
                        .tx_ref()
                        .put::<tables::StorageChangeSets>(
                            BlockNumberAddress((block_number as BlockNumber, address(a))),
                            StorageEntry { key: slot(s), value: U256::ZERO },
                        )
                        .unwrap();
                }
            }
        }
        provider.commit().unwrap();

        let provider = factory.provider().unwrap();
        let last_block = account_blocks.len().max(storage_blocks.len()) as BlockNumber - 1;
        for range in [0..=last_block, 1.min(last_block)..=last_block, 0..=last_block + 2] {
            let in_range = |block_number: usize| range.contains(&(block_number as BlockNumber));
            let mut expected = TriePrefixSetsMut::default();
            for (_, accounts) in account_blocks.iter().enumerate().filter(|(n, _)| in_range(*n)) {
                for &a in accounts {
                    let hashed_address = keccak256(address(a));
                    expected.account_prefix_set.insert(Nibbles::unpack(hashed_address));
                    if !existing.contains(&a) {
                        expected.destroyed_accounts.insert(hashed_address);
                    }
                }
            }
            for (_, slots) in storage_blocks.iter().enumerate().filter(|(n, _)| in_range(*n)) {
                for &(a, s) in slots {
                    let hashed_address = keccak256(address(a));
                    expected.account_prefix_set.insert(Nibbles::unpack(hashed_address));
                    expected
                        .storage_prefix_sets
                        .entry(hashed_address)
                        .or_default()
                        .insert(Nibbles::unpack(keccak256(slot(s))));
                }
            }

            let actual = load_prefix_sets_with_provider(&provider, range).unwrap();
            assert_prefix_sets_eq(&actual, &expected.freeze());
        }
    }

    // Few distinct indices, so addresses and slots repeat across blocks.
    fn account_blocks_strategy() -> impl Strategy<Value = AccountBlocks> {
        prop::collection::vec(btree_set(0u8..12, 0..6), 1..12)
    }

    fn storage_blocks_strategy() -> impl Strategy<Value = StorageBlocks> {
        prop::collection::vec(btree_set((0u8..12, 0u8..6), 0..10), 1..12)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn matches_fixture_v1(
            account_blocks in account_blocks_strategy(),
            storage_blocks in storage_blocks_strategy(),
            existing in btree_set(0u8..12, 0..12),
        ) {
            check_against_fixture(&account_blocks, &storage_blocks, &existing, false);
        }

        #[test]
        fn matches_fixture_v2(
            account_blocks in account_blocks_strategy(),
            storage_blocks in storage_blocks_strategy(),
            existing in btree_set(0u8..12, 0..12),
        ) {
            check_against_fixture(&account_blocks, &storage_blocks, &existing, true);
        }
    }
}
