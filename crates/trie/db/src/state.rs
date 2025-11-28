use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, PrefixSetLoader};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    BlockNumber, B256, U256,
};
use reth_db_api::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress, BlockNumberAddressRange},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_execution_errors::StateRootError;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates, HashedPostState, HashedPostStateSorted, HashedStorage,
    HashedStorageSorted, KeccakKeyHasher, KeyHasher, StateRoot, StateRootProgress, TrieInput,
};
use std::{
    collections::{BTreeMap, HashMap},
    ops::{RangeBounds, RangeInclusive},
    time::{Duration, Instant},
};
use tracing::{debug, instrument};

/// Extends [`StateRoot`] with operations specific for working with a database transaction.
pub trait DatabaseStateRoot<'a, TX>: Sized {
    /// Create a new [`StateRoot`] instance.
    fn from_tx(tx: &'a TX) -> Self;

    /// Given a block number range, identifies all the accounts and storage keys that
    /// have changed.
    ///
    /// # Returns
    ///
    /// An instance of state root calculator with account and storage prefixes loaded.
    fn incremental_root_calculator(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError>;

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes.
    ///
    /// # Returns
    ///
    /// The updated state root.
    fn incremental_root(
        tx: &'a TX,
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
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    fn incremental_root_with_progress(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError>;

    /// Calculate the state root for this [`HashedPostState`].
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
    /// let state_root = StateRoot::overlay_root(&tx, hashed_state);
    /// ```
    ///
    /// # Returns
    ///
    /// The state root for this [`HashedPostState`].
    fn overlay_root(tx: &'a TX, post_state: HashedPostState) -> Result<B256, StateRootError>;

    /// Calculates the state root for this [`HashedPostState`] and returns it alongside trie
    /// updates. See [`Self::overlay_root`] for more info.
    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostState`] using cached intermediate nodes.
    fn overlay_root_from_nodes(tx: &'a TX, input: TrieInput) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostState`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError>;
}

/// Extends [`HashedPostState`] with operations specific for working with a database transaction.
pub trait DatabaseHashedPostState<TX>: Sized {
    /// Initializes [`HashedPostState`] from reverts. Iterates over state reverts in the specified
    /// range and aggregates them into hashed state in reverse.
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Self, DatabaseError>;
}

/// Statistics for `from_reverts` operation.
#[derive(Clone, Copy, Debug)]
pub struct FromRevertsStats {
    /// Total duration for the operation.
    duration: Duration,
    /// Time spent collecting data from changesets.
    collection_duration: Duration,
    /// Time spent sorting/converting to final structure.
    sorting_duration: Duration,
    /// Number of accounts processed.
    accounts_count: u64,
    /// Number of storage entries processed.
    storages_count: u64,
}

impl FromRevertsStats {
    /// Total duration for the operation.
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Time spent collecting data from changesets.
    pub const fn collection_duration(&self) -> Duration {
        self.collection_duration
    }

    /// Time spent sorting/converting to final structure.
    pub const fn sorting_duration(&self) -> Duration {
        self.sorting_duration
    }

    /// Number of accounts processed.
    pub const fn accounts_count(&self) -> u64 {
        self.accounts_count
    }

    /// Number of storage entries processed.
    pub const fn storages_count(&self) -> u64 {
        self.storages_count
    }
}

/// Tracker for `from_reverts` metrics.
#[derive(Debug)]
pub struct FromRevertsTracker {
    started_at: Instant,
    collection_duration: Duration,
    accounts_count: u64,
    storages_count: u64,
}

impl Default for FromRevertsTracker {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            collection_duration: Duration::ZERO,
            accounts_count: 0,
            storages_count: 0,
        }
    }
}

impl FromRevertsTracker {
    /// Record the end of collection phase.
    pub fn finish_collection(&mut self) {
        self.collection_duration = self.started_at.elapsed();
    }

    /// Set the accounts count.
    pub const fn set_accounts_count(&mut self, count: u64) {
        self.accounts_count = count;
    }

    /// Set the storages count.
    pub const fn set_storages_count(&mut self, count: u64) {
        self.storages_count = count;
    }

    /// Called when operation is finished to return statistics.
    pub fn finish(self) -> FromRevertsStats {
        let total_duration = self.started_at.elapsed();
        FromRevertsStats {
            duration: total_duration,
            collection_duration: self.collection_duration,
            sorting_duration: total_duration.saturating_sub(self.collection_duration),
            accounts_count: self.accounts_count,
            storages_count: self.storages_count,
        }
    }
}

/// Extends [`HashedPostStateSorted`] with operations specific for working with a database
/// transaction.
pub trait DatabaseHashedPostStateSorted<TX>: Sized {
    /// Initializes [`HashedPostStateSorted`] from reverts. Iterates over state reverts in the
    /// specified range and aggregates them into sorted hashed state.
    /// Returns the sorted state along with performance statistics.
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<(Self, FromRevertsStats), DatabaseError>;
}

impl<'a, TX: DbTx> DatabaseStateRoot<'a, TX>
    for StateRoot<DatabaseTrieCursorFactory<&'a TX>, DatabaseHashedCursorFactory<&'a TX>>
{
    fn from_tx(tx: &'a TX) -> Self {
        Self::new(DatabaseTrieCursorFactory::new(tx), DatabaseHashedCursorFactory::new(tx))
    }

    fn incremental_root_calculator(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, StateRootError> {
        let loaded_prefix_sets = PrefixSetLoader::<_, KeccakKeyHasher>::new(tx).load(range)?;
        Ok(Self::from_tx(tx).with_prefix_sets(loaded_prefix_sets))
    }

    fn incremental_root(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<B256, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root()
    }

    fn incremental_root_with_updates(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        Self::incremental_root_calculator(tx, range)?.root_with_updates()
    }

    fn incremental_root_with_progress(
        tx: &'a TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root with progress");
        Self::incremental_root_calculator(tx, range)?.root_with_progress()
    }

    fn overlay_root(tx: &'a TX, post_state: HashedPostState) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        let state_sorted = post_state.into_sorted();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(prefix_sets)
        .root_with_updates()
    }

    fn overlay_root_from_nodes(tx: &'a TX, input: TrieInput) -> Result<B256, StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), &nodes_sorted),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()
    }

    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let state_sorted = input.state.into_sorted();
        let nodes_sorted = input.nodes.into_sorted();
        StateRoot::new(
            InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(tx), &nodes_sorted),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), &state_sorted),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()
    }
}

impl<TX: DbTx> DatabaseHashedPostState<TX> for HashedPostState {
    #[instrument(target = "trie::db", skip(tx), fields(range))]
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Self, DatabaseError> {
        // Iterate over account changesets and record value before first occurring account change.
        let account_range = (range.start_bound(), range.end_bound()); // to avoid cloning
        let mut accounts = HashMap::new();
        let mut account_changesets_cursor = tx.cursor_read::<tables::AccountChangeSets>()?;
        for entry in account_changesets_cursor.walk_range(account_range)? {
            let (_, AccountBeforeTx { address, info }) = entry?;
            accounts.entry(address).or_insert(info);
        }

        // Iterate over storage changesets and record value before first occurring storage change.
        let storage_range: BlockNumberAddressRange = range.into();
        let mut storages = AddressMap::<B256Map<U256>>::default();
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSets>()?;
        for entry in storage_changesets_cursor.walk_range(storage_range)? {
            let (BlockNumberAddress((_, address)), storage) = entry?;
            let account_storage = storages.entry(address).or_default();
            account_storage.entry(storage.key).or_insert(storage.value);
        }

        let hashed_accounts =
            accounts.into_iter().map(|(address, info)| (KH::hash_key(address), info)).collect();

        let hashed_storages = storages
            .into_iter()
            .map(|(address, storage)| {
                (
                    KH::hash_key(address),
                    HashedStorage::from_iter(
                        // The `wiped` flag indicates only whether previous storage entries
                        // should be looked up in db or not. For reverts it's a noop since all
                        // wiped changes had been written as storage reverts.
                        false,
                        storage.into_iter().map(|(slot, value)| (KH::hash_key(slot), value)),
                    ),
                )
            })
            .collect();

        Ok(Self { accounts: hashed_accounts, storages: hashed_storages })
    }
}

impl<TX: DbTx> DatabaseHashedPostStateSorted<TX> for HashedPostStateSorted {
    #[instrument(target = "trie::db", skip(tx), fields(range))]
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<(Self, FromRevertsStats), DatabaseError> {
        let mut tracker = FromRevertsTracker::default();

        // Use BTreeMap for automatic sorting during insertion (hash immediately)
        let account_range = (range.start_bound(), range.end_bound());
        let mut accounts: BTreeMap<B256, Option<reth_primitives_traits::Account>> = BTreeMap::new();
        let mut account_changesets_cursor = tx.cursor_read::<tables::AccountChangeSets>()?;
        for entry in account_changesets_cursor.walk_range(account_range)? {
            let (_, AccountBeforeTx { address, info }) = entry?;
            accounts.entry(KH::hash_key(address)).or_insert(info);
        }

        let storage_range: BlockNumberAddressRange = range.into();
        let mut storages: BTreeMap<B256, BTreeMap<B256, U256>> = BTreeMap::new();
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSets>()?;
        for entry in storage_changesets_cursor.walk_range(storage_range)? {
            let (BlockNumberAddress((_, address)), storage) = entry?;
            let hashed_address = KH::hash_key(address);
            let hashed_slot = KH::hash_key(storage.key);
            storages.entry(hashed_address).or_default().entry(hashed_slot).or_insert(storage.value);
        }

        // Mark end of collection phase (includes hashing + BTreeMap insertions)
        tracker.finish_collection();

        // Convert BTreeMap to Vec (already sorted, just collect)
        let sorted_accounts: Vec<_> = accounts.into_iter().collect();
        let sorted_storages: B256Map<HashedStorageSorted> = storages
            .into_iter()
            .map(|(addr, slots)| {
                (
                    addr,
                    HashedStorageSorted {
                        storage_slots: slots.into_iter().collect(),
                        wiped: false,
                    },
                )
            })
            .collect();

        tracker.set_accounts_count(sorted_accounts.len() as u64);
        tracker.set_storages_count(sorted_storages.len() as u64);

        let stats = tracker.finish();

        tracing::trace!(
            target: "trie::db",
            accounts_count = stats.accounts_count(),
            storages_count = stats.storages_count(),
            ?stats,
            "from_reverts_sorted (btree)"
        );

        Ok((Self::new(sorted_accounts, sorted_storages), stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, map::HashMap, Address, U256};
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::{
        cursor::DbCursorRW,
        database::Database,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_trie::KeccakKeyHasher;
    use revm::state::AccountInfo;
    use revm_database::BundleState;

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
            StateRoot::overlay_root(&tx, post_state).unwrap(),
            hex!("b464525710cafcf5d4044ac85b72c08b1e76231b8d91f288fe438cc41d8eaafd")
        );
    }

    #[test]
    fn from_reverts_sorted_is_actually_sorted() {
        // Create a test database with some account and storage changesets
        let db = create_test_rw_db();

        // Setup test data - addresses must be sorted for storage changesets
        // (storage changesets are keyed by BlockNumberAddress which sorts by block then address)
        let addresses =
            [Address::with_last_byte(1), Address::with_last_byte(2), Address::with_last_byte(3)];
        let accounts = [
            Some(Account { nonce: 1, balance: U256::from(100), bytecode_hash: None }),
            Some(Account { nonce: 2, balance: U256::from(200), bytecode_hash: None }),
            None, // destroyed account
        ];
        // Multiple slots per address to test storage sorting
        let slot1 = B256::from([1u8; 32]);
        let slot2 = B256::from([2u8; 32]);
        let slot3 = B256::from([3u8; 32]);

        // Write changesets
        {
            let tx = db.tx_mut().expect("failed to create transaction");
            let mut account_cursor =
                tx.cursor_write::<tables::AccountChangeSets>().expect("failed to create cursor");
            let mut storage_cursor =
                tx.cursor_write::<tables::StorageChangeSets>().expect("failed to create cursor");

            // Write account changesets for block 5 (can be any order, they're keyed by block)
            for (addr, info) in addresses.iter().zip(accounts.iter()) {
                account_cursor
                    .append(5, &AccountBeforeTx { address: *addr, info: *info })
                    .expect("failed to write account changeset");
            }

            // Write storage changesets for block 5 - must be sorted by (block, address)
            // First address gets multiple slots
            storage_cursor
                .append(
                    BlockNumberAddress((5, addresses[0])),
                    &StorageEntry { key: slot3, value: U256::from(30) },
                )
                .expect("failed to write storage changeset 1");
            storage_cursor
                .append(
                    BlockNumberAddress((5, addresses[1])),
                    &StorageEntry { key: slot1, value: U256::from(10) },
                )
                .expect("failed to write storage changeset 2");
            storage_cursor
                .append(
                    BlockNumberAddress((5, addresses[2])),
                    &StorageEntry { key: slot2, value: U256::from(20) },
                )
                .expect("failed to write storage changeset 3");

            tx.commit().expect("failed to commit");
        }

        // Now read using from_reverts_sorted
        let tx = db.tx().expect("failed to create transaction");
        let (sorted, stats) =
            HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, 5..6).unwrap();

        // Verify stats
        assert_eq!(stats.accounts_count(), 3);
        assert_eq!(stats.storages_count(), 3);
        assert!(stats.duration() > Duration::ZERO);

        // Verify accounts are sorted by hashed address
        for window in sorted.accounts.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "accounts should be sorted: {:?} should be < {:?}",
                window[0].0,
                window[1].0
            );
        }

        // Verify storage slots are sorted within each account
        for (_, storage) in &sorted.storages {
            for window in storage.storage_slots.windows(2) {
                assert!(
                    window[0].0 < window[1].0,
                    "storage slots should be sorted: {:?} should be < {:?}",
                    window[0].0,
                    window[1].0
                );
            }
        }
    }

    #[test]
    fn from_reverts_sorted_stats_timing() {
        let db = create_test_rw_db();
        let tx = db.tx().expect("failed to create transaction");

        // Even with empty changesets, stats should be valid
        let (sorted, stats) =
            HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, 1..10).unwrap();

        assert!(sorted.accounts.is_empty());
        assert!(sorted.storages.is_empty());
        assert_eq!(stats.accounts_count(), 0);
        assert_eq!(stats.storages_count(), 0);
        // Duration should be non-negative (might be zero on fast systems)
        assert!(stats.duration() >= Duration::ZERO);
        assert!(stats.collection_duration() >= Duration::ZERO);
        assert!(stats.sorting_duration() >= Duration::ZERO);
    }
}
