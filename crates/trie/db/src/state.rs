use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use alloy_primitives::{keccak256, map::B256Map, BlockNumber, B256};
use reth_db_api::{
    models::{AccountBeforeTx, BlockNumberAddress},
    transaction::DbTx,
};
use reth_execution_errors::StateRootError;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates, HashedPostStateSorted, HashedStorageSorted, StateRoot, StateRootProgress,
    TrieInputSorted,
};
use std::{
    collections::HashSet,
    ops::{Bound, RangeBounds, RangeInclusive},
};
use tracing::{debug, instrument};

/// Extends [`StateRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStateRoot<'a, TX>: Sized {
    /// Create a new [`StateRoot`] instance.
    fn from_tx(tx: &'a TX, is_v2: bool) -> Self;

    /// Given a block number range, identifies all the accounts and storage keys that
    /// have changed.
    ///
    /// # Returns
    ///
    /// An instance of state root calculator with account and storage prefixes loaded.
    fn incremental_root_calculator(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError>;

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes.
    ///
    /// # Returns
    ///
    /// The updated state root.
    fn incremental_root(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<B256, StateRootError>;

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// Ignores the threshold.
    ///
    /// # Returns
    ///
    /// The updated state root and the trie updates.
    fn incremental_root_with_updates(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    fn incremental_root_with_progress(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError>;

    /// Calculate the state root for this [`HashedPostStateSorted`].
    /// Internally, this method retrieves prefixsets and uses them
    /// to calculate incremental state root.
    ///
    /// # Example
    ///
    /// ```
    /// use alloy_primitives::U256;
    /// use reth_db::test_utils::create_test_rw_db;
    /// use reth_db_api::database::Database;
    /// use reth_primitives_traits::Account;
    /// use reth_trie::{updates::TrieUpdates, HashedPostState, StateRoot};
    /// use reth_trie_db::DatabaseStateRoot;
    ///
    /// // Initialize the database
    /// let db = create_test_rw_db();
    ///
    /// // Initialize hashed post state
    /// let mut hashed_state = HashedPostState::default();
    /// hashed_state.accounts.insert(
    ///     [0x11; 32].into(),
    ///     Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None }),
    /// );
    ///
    /// // Calculate the state root
    /// let tx = db.tx().expect("failed to create transaction");
    /// let state_root = <StateRoot<
    ///     reth_trie_db::DatabaseTrieCursorFactory<_>,
    ///     reth_trie_db::DatabaseHashedCursorFactory<_>,
    /// > as DatabaseStateRoot<_>>::overlay_root(&tx, &hashed_state.into_sorted(), false);
    /// ```
    ///
    /// # Returns
    ///
    /// The state root for this [`HashedPostStateSorted`].
    fn overlay_root(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
        is_v2: bool,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root for this [`HashedPostStateSorted`] and returns it alongside trie
    /// updates. See [`Self::overlay_root`] for more info.
    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
        is_v2: bool,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostStateSorted`] using cached intermediate
    /// nodes.
    fn overlay_root_from_nodes(
        tx: &'a TX,
        input: TrieInputSorted,
        is_v2: bool,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostStateSorted`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInputSorted,
        is_v2: bool,
    ) -> Result<(B256, TrieUpdates), StateRootError>;
}

/// Extends [`HashedPostStateSorted`] with operations specific for working with a database
/// transaction.
pub trait DatabaseHashedPostState: Sized {
    /// Initializes [`HashedPostStateSorted`] from reverts. Iterates over state reverts in the
    /// specified range and aggregates them into sorted hashed state.
    ///
    /// Storage keys from changesets are tagged as
    /// [`Plain`](reth_primitives_traits::StorageSlotKey::Plain) or
    /// [`Hashed`](reth_primitives_traits::StorageSlotKey::Hashed) by the reader, so no
    /// `use_hashed_state` flag is needed. Addresses are always hashed.
    fn from_reverts(
        provider: &(impl ChangeSetReader + StorageChangeSetReader + BlockNumReader + DBProvider),
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<HashedPostStateSorted, ProviderError>;
}

impl<'a, TX: DbTx> DatabaseStateRoot<'a, TX>
    for StateRoot<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX, is_v2: bool) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx, is_v2), DatabaseHashedCursorFactory::new(tx))
    }

    fn incremental_root_calculator(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError> {
        let loaded_prefix_sets =
            crate::prefix_set::load_prefix_sets_with_provider(provider, range)?;
        let is_v2 = provider.cached_storage_settings().is_v2();
        Ok(Self::from_tx(provider.tx_ref(), is_v2).with_prefix_sets(loaded_prefix_sets))
    }

    fn incremental_root(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<B256, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(provider, range)?.root()
    }

    fn incremental_root_with_updates(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(provider, range)?.root_with_updates()
    }

    fn incremental_root_with_progress(
        provider: &'a (impl ChangeSetReader
                 + StorageChangeSetReader
                 + StorageSettingsCache
                 + DBProvider<Tx = TX>),
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root with progress");
        Self::incremental_root_calculator(provider, range)?.root_with_progress()
    }

    fn overlay_root(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
        is_v2: bool,
    ) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx, is_v2),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), post_state),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
        is_v2: bool,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx, is_v2),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), post_state),
        )
        .with_prefix_sets(prefix_sets)
        .root_with_updates()
    }

    fn overlay_root_from_nodes(
        tx: &'a TX,
        input: TrieInputSorted,
        is_v2: bool,
    ) -> Result<B256, StateRootError> {
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx, is_v2),
                input.nodes.as_ref(),
            ),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(tx),
                input.state.as_ref(),
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()
    }

    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInputSorted,
        is_v2: bool,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx, is_v2),
                input.nodes.as_ref(),
            ),
            HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(tx),
                input.state.as_ref(),
            ),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()
    }
}

/// Calls [`HashedPostStateSorted::from_reverts`].
///
/// This is a convenience wrapper kept for backward compatibility. The storage
/// key tagging is now handled internally by the changeset reader.
pub fn from_reverts_auto(
    provider: &(impl ChangeSetReader
          + StorageChangeSetReader
          + BlockNumReader
          + DBProvider
          + StorageSettingsCache),
    range: impl RangeBounds<BlockNumber>,
) -> Result<HashedPostStateSorted, ProviderError> {
    HashedPostStateSorted::from_reverts(provider, range)
}

impl DatabaseHashedPostState for HashedPostStateSorted {
    /// Builds a sorted hashed post-state from reverts.
    ///
    /// Reads MDBX data directly into Vecs, using `HashSet`s only to track seen keys.
    /// This avoids intermediate `HashMap` allocations since MDBX data is already sorted.
    ///
    /// - Reads the first occurrence of each changed account/storage slot in the range.
    /// - Addresses are always keccak256-hashed.
    /// - Storage keys are tagged by the changeset reader and hashed via
    ///   [`StorageSlotKey::to_hashed`](reth_primitives_traits::StorageSlotKey::to_hashed).
    /// - Returns keys already ordered for trie iteration.
    #[instrument(target = "trie::db", skip(provider), fields(range))]
    fn from_reverts(
        provider: &(impl ChangeSetReader + StorageChangeSetReader + BlockNumReader + DBProvider),
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Self, ProviderError> {
        // Extract concrete start/end values to use for both account and storage changesets.
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            Bound::Included(&n) => n + 1,
            Bound::Excluded(&n) => n,
            Bound::Unbounded => BlockNumber::MAX,
        };

        // Iterate over account changesets and record value before first occurring account change
        let mut accounts = Vec::new();
        let mut seen_accounts = HashSet::new();
        for entry in provider.account_changesets_range(start..end)? {
            let (_, AccountBeforeTx { address, info }) = entry;
            if seen_accounts.insert(address) {
                accounts.push((keccak256(address), info));
            }
        }
        accounts.sort_unstable_by_key(|(hash, _)| *hash);

        // Read storages into B256Map<Vec<_>> with HashSet to track seen keys.
        // Only keep the first (oldest) occurrence of each (address, slot) pair.
        let mut storages = B256Map::<Vec<_>>::default();
        let mut seen_storage_keys = HashSet::new();

        if start < end {
            let end_inclusive = end.saturating_sub(1);
            for (BlockNumberAddress((_, address)), storage) in
                provider.storage_changesets_range(start..=end_inclusive)?
            {
                if seen_storage_keys.insert((address, storage.key.as_b256())) {
                    let hashed_address = keccak256(address);
                    storages
                        .entry(hashed_address)
                        .or_default()
                        .push((storage.key.to_hashed(), storage.value));
                }
            }
        }

        // Sort storage slots and convert to HashedStorageSorted
        let hashed_storages = storages
            .into_iter()
            .map(|(address, mut slots)| {
                slots.sort_unstable_by_key(|(slot, _)| *slot);
                (address, HashedStorageSorted { storage_slots: slots, wiped: false })
            })
            .collect();

        Ok(Self::new(accounts, hashed_storages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, keccak256, map::HashMap, Address, B256, U256};
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::{
        database::Database,
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_trie::{HashedPostState, HashedStorage, KeccakKeyHasher, StateRoot};
    use revm::state::AccountInfo;
    use revm_database::BundleState;

    type DbStateRoot<'a, TX> = StateRoot<
        crate::DatabaseTrieCursorFactory<&'a TX>,
        crate::DatabaseHashedCursorFactory<&'a TX>,
    >;

    /// Overlay root calculation works with sorted state.
    #[test]
    fn overlay_root_with_sorted_state() {
        let db = create_test_rw_db();
        let tx = db.tx().expect("failed to create transaction");

        let mut hashed_state = HashedPostState::default();
        hashed_state.accounts.insert(
            B256::from(U256::from(1)),
            Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None }),
        );
        hashed_state.accounts.insert(B256::from(U256::from(2)), None);
        hashed_state.storages.insert(
            B256::from(U256::from(1)),
            HashedStorage::from_iter(false, [(B256::from(U256::from(3)), U256::from(30))]),
        );

        let sorted = hashed_state.into_sorted();
        let overlay_root = DbStateRoot::overlay_root(&tx, &sorted, false).unwrap();

        // Just verify it produces a valid root
        assert!(!overlay_root.is_zero());
    }

    /// Builds hashed state from a bundle and checks the known state root.
    #[test]
    fn from_bundle_state_with_rayon() {
        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let slot1 = U256::from(1015);
        let slot2 = U256::from(2015);

        let account1 = AccountInfo { nonce: 1, ..Default::default() };
        let account2 = AccountInfo { nonce: 2, ..Default::default() };

        let bundle_state = BundleState::builder(2..=2)
            .state_present_account_info(address1, account1)
            .state_present_account_info(address2, account2)
            .state_storage(address1, HashMap::from_iter([(slot1, (U256::ZERO, U256::from(10)))]))
            .state_storage(address2, HashMap::from_iter([(slot2, (U256::ZERO, U256::from(20)))]))
            .build();
        assert_eq!(bundle_state.reverts.len(), 1);

        let post_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(&bundle_state.state);
        assert_eq!(post_state.accounts.len(), 2);
        assert_eq!(post_state.storages.len(), 2);

        let db = create_test_rw_db();
        let tx = db.tx().expect("failed to create transaction");
        assert_eq!(
            DbStateRoot::overlay_root(&tx, &post_state.into_sorted(), false).unwrap(),
            hex!("b464525710cafcf5d4044ac85b72c08b1e76231b8d91f288fe438cc41d8eaafd")
        );
    }

    /// Verifies `from_reverts` keeps first occurrence per key and preserves ordering guarantees.
    #[test]
    fn from_reverts_keeps_first_occurrence_and_ordering() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let slot1 = B256::from(U256::from(11));
        let slot2 = B256::from(U256::from(22));

        // Account changesets: only first occurrence per address should be kept.
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                1,
                AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 1, ..Default::default() }),
                },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                2,
                AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 2, ..Default::default() }),
                },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(3, AccountBeforeTx { address: address2, info: None })
            .unwrap();

        // Storage changesets: only first occurrence per slot should be kept, and slots sorted.
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((1, address1)),
                StorageEntry { key: slot2, value: U256::from(200) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address1)),
                StorageEntry { key: slot1, value: U256::from(100) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address1)),
                StorageEntry { key: slot1, value: U256::from(999) }, // should be ignored
            )
            .unwrap();

        let sorted = HashedPostStateSorted::from_reverts(&*provider, 1..=3).unwrap();

        // Verify first occurrences were kept (nonce 1, not 2)
        assert_eq!(sorted.accounts.len(), 2);
        let hashed_addr1 = keccak256(address1);
        let account1 = sorted.accounts.iter().find(|(addr, _)| *addr == hashed_addr1).unwrap();
        assert_eq!(account1.1.unwrap().nonce, 1);

        // Ordering guarantees - accounts sorted by hashed address
        assert!(sorted.accounts.windows(2).all(|w| w[0].0 <= w[1].0));

        // Ordering guarantees - storage slots sorted by hashed slot
        for storage in sorted.storages.values() {
            assert!(storage.storage_slots.windows(2).all(|w| w[0].0 <= w[1].0));
        }
    }

    /// Empty block range returns empty state.
    #[test]
    fn from_reverts_empty_range() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        // Insert data outside the query range
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                100,
                AccountBeforeTx {
                    address: Address::with_last_byte(1),
                    info: Some(Account { nonce: 1, ..Default::default() }),
                },
            )
            .unwrap();

        // Query a range with no data
        let sorted = HashedPostStateSorted::from_reverts(&*provider, 1..=10).unwrap();
        assert!(sorted.accounts.is_empty());
        assert!(sorted.storages.is_empty());
    }

    #[test]
    fn from_reverts_with_hashed_state() {
        use reth_db_api::models::{StorageBeforeTx, StorageSettings};
        use reth_provider::{StaticFileProviderFactory, StaticFileSegment, StaticFileWriter};

        let factory = create_test_provider_factory();

        factory.set_storage_settings_cache(StorageSettings::v2());

        let provider = factory.provider_rw().unwrap();

        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);

        let plain_slot1 = B256::from(U256::from(11));
        let plain_slot2 = B256::from(U256::from(22));
        let hashed_slot1 = keccak256(plain_slot1);
        let hashed_slot2 = keccak256(plain_slot2);

        {
            let sf = factory.static_file_provider();

            // Write account changesets to static files (v2 reads from here)
            let mut aw = sf.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            aw.append_account_changeset(vec![], 0).unwrap();
            aw.append_account_changeset(
                vec![AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 1, ..Default::default() }),
                }],
                1,
            )
            .unwrap();
            aw.append_account_changeset(
                vec![AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 2, ..Default::default() }),
                }],
                2,
            )
            .unwrap();
            aw.append_account_changeset(vec![AccountBeforeTx { address: address2, info: None }], 3)
                .unwrap();
            aw.commit().unwrap();

            let mut writer = sf.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            writer.append_storage_changeset(vec![], 0).unwrap();
            writer
                .append_storage_changeset(
                    vec![StorageBeforeTx {
                        address: address1,
                        key: hashed_slot2,
                        value: U256::from(200),
                    }],
                    1,
                )
                .unwrap();
            writer
                .append_storage_changeset(
                    vec![StorageBeforeTx {
                        address: address1,
                        key: hashed_slot1,
                        value: U256::from(100),
                    }],
                    2,
                )
                .unwrap();
            writer
                .append_storage_changeset(
                    vec![StorageBeforeTx {
                        address: address1,
                        key: hashed_slot1,
                        value: U256::from(999),
                    }],
                    3,
                )
                .unwrap();
            writer.commit().unwrap();
        }

        let sorted = HashedPostStateSorted::from_reverts(&*provider, 1..=3).unwrap();

        assert_eq!(sorted.accounts.len(), 2);

        let hashed_addr1 = keccak256(address1);
        let hashed_addr2 = keccak256(address2);

        let account1 = sorted.accounts.iter().find(|(addr, _)| *addr == hashed_addr1).unwrap();
        assert_eq!(account1.1.unwrap().nonce, 1);

        let account2 = sorted.accounts.iter().find(|(addr, _)| *addr == hashed_addr2).unwrap();
        assert!(account2.1.is_none());

        assert!(sorted.accounts.windows(2).all(|w| w[0].0 <= w[1].0));

        let storage = sorted.storages.get(&hashed_addr1).expect("storage for address1");
        assert_eq!(storage.storage_slots.len(), 2);

        let found_slot1 = storage.storage_slots.iter().find(|(k, _)| *k == hashed_slot1).unwrap();
        assert_eq!(found_slot1.1, U256::from(100));

        let found_slot2 = storage.storage_slots.iter().find(|(k, _)| *k == hashed_slot2).unwrap();
        assert_eq!(found_slot2.1, U256::from(200));

        assert_ne!(hashed_slot1, plain_slot1);
        assert_ne!(hashed_slot2, plain_slot2);

        assert!(storage.storage_slots.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn from_reverts_legacy_keccak_hashes_all_keys() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let plain_slot1 = B256::from(U256::from(11));
        let plain_slot2 = B256::from(U256::from(22));

        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                1,
                AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 10, ..Default::default() }),
                },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                2,
                AccountBeforeTx {
                    address: address2,
                    info: Some(Account { nonce: 20, ..Default::default() }),
                },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                3,
                AccountBeforeTx {
                    address: address1,
                    info: Some(Account { nonce: 99, ..Default::default() }),
                },
            )
            .unwrap();

        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((1, address1)),
                StorageEntry { key: plain_slot1, value: U256::from(100) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address1)),
                StorageEntry { key: plain_slot2, value: U256::from(200) },
            )
            .unwrap();
        provider
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((3, address2)),
                StorageEntry { key: plain_slot1, value: U256::from(300) },
            )
            .unwrap();

        let sorted = HashedPostStateSorted::from_reverts(&*provider, 1..=3).unwrap();

        let expected_hashed_addr1 = keccak256(address1);
        let expected_hashed_addr2 = keccak256(address2);
        assert_eq!(sorted.accounts.len(), 2);

        let account1 =
            sorted.accounts.iter().find(|(addr, _)| *addr == expected_hashed_addr1).unwrap();
        assert_eq!(account1.1.unwrap().nonce, 10);

        let account2 =
            sorted.accounts.iter().find(|(addr, _)| *addr == expected_hashed_addr2).unwrap();
        assert_eq!(account2.1.unwrap().nonce, 20);

        assert!(sorted.accounts.windows(2).all(|w| w[0].0 <= w[1].0));

        let expected_hashed_slot1 = keccak256(plain_slot1);
        let expected_hashed_slot2 = keccak256(plain_slot2);

        assert_ne!(expected_hashed_slot1, plain_slot1);
        assert_ne!(expected_hashed_slot2, plain_slot2);

        let storage1 = sorted.storages.get(&expected_hashed_addr1).expect("storage for address1");
        assert_eq!(storage1.storage_slots.len(), 2);
        assert!(storage1
            .storage_slots
            .iter()
            .any(|(k, v)| *k == expected_hashed_slot1 && *v == U256::from(100)));
        assert!(storage1
            .storage_slots
            .iter()
            .any(|(k, v)| *k == expected_hashed_slot2 && *v == U256::from(200)));
        assert!(storage1.storage_slots.windows(2).all(|w| w[0].0 <= w[1].0));

        let storage2 = sorted.storages.get(&expected_hashed_addr2).expect("storage for address2");
        assert_eq!(storage2.storage_slots.len(), 1);
        assert_eq!(storage2.storage_slots[0].0, expected_hashed_slot1);
        assert_eq!(storage2.storage_slots[0].1, U256::from(300));
    }
}
