use crate::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, PrefixSetLoader};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    Address, BlockNumber, B256, U256,
};
use reth_db_api::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress, BlockNumberAddressRange},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_execution_errors::StateRootError;
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates, HashedPostStateSorted, HashedStorageSorted, KeccakKeyHasher, KeyHasher,
    StateRoot, StateRootProgress, TrieInput, TrieInputSorted,
};
use std::{
    collections::HashMap,
    ops::{RangeBounds, RangeInclusive},
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
    /// let state_root = StateRoot::overlay_root(&tx, &hashed_state.into_sorted());
    /// ```
    ///
    /// # Returns
    ///
    /// The state root for this [`HashedPostStateSorted`].
    fn overlay_root(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root for this [`HashedPostStateSorted`] and returns it alongside trie
    /// updates. See [`Self::overlay_root`] for more info.
    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostState`] using cached intermediate nodes.
    fn overlay_root_from_nodes(tx: &'a TX, input: TrieInput) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostState`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_with_updates(
        tx: &'a TX,
        input: TrieInput,
    ) -> Result<(B256, TrieUpdates), StateRootError>;

    /// Calculates the state root for provided [`HashedPostStateSorted`] using cached intermediate
    /// nodes.
    fn overlay_root_from_nodes_sorted(
        tx: &'a TX,
        input: TrieInputSorted,
    ) -> Result<B256, StateRootError>;

    /// Calculates the state root and trie updates for provided [`HashedPostStateSorted`] using
    /// cached intermediate nodes.
    fn overlay_root_from_nodes_sorted_with_updates(
        tx: &'a TX,
        input: TrieInputSorted,
    ) -> Result<(B256, TrieUpdates), StateRootError>;
}

/// Extends [`HashedPostStateSorted`] with operations specific for working with a database
/// transaction.
pub trait DatabaseHashedPostState<TX>: Sized {
    /// Initializes [`HashedPostStateSorted`] from reverts. Iterates over state reverts in the
    /// specified range and aggregates them into sorted hashed state.
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<HashedPostStateSorted, DatabaseError>;
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

    fn overlay_root(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
    ) -> Result<B256, StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), post_state),
        )
        .with_prefix_sets(prefix_sets)
        .root()
    }

    fn overlay_root_with_updates(
        tx: &'a TX,
        post_state: &HashedPostStateSorted,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let prefix_sets = post_state.construct_prefix_sets().freeze();
        StateRoot::new(
            DatabaseTrieCursorFactory::new(tx),
            HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(tx), post_state),
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

    fn overlay_root_from_nodes_sorted(
        tx: &'a TX,
        input: TrieInputSorted,
    ) -> Result<B256, StateRootError> {
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx),
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

    fn overlay_root_from_nodes_sorted_with_updates(
        tx: &'a TX,
        input: TrieInputSorted,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        StateRoot::new(
            InMemoryTrieCursorFactory::new(
                DatabaseTrieCursorFactory::new(tx),
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

/// Reads account and storage reverts from database changesets.
///
/// Iterates over account and storage changesets in the given block range,
/// keeping only the first occurrence of each account/slot (the value before
/// any changes in that range).
///
/// Returns raw (unhashed) data for further processing.
#[allow(clippy::type_complexity)]
fn read_reverts_from_changesets<TX: DbTx>(
    tx: &TX,
    range: impl RangeBounds<BlockNumber>,
) -> Result<(HashMap<Address, Option<Account>>, AddressMap<B256Map<U256>>), DatabaseError> {
    // Iterate over account changesets and record value before first occurring account change.
    let account_range = (range.start_bound(), range.end_bound());
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

    Ok((accounts, storages))
}

impl<TX: DbTx> DatabaseHashedPostState<TX> for HashedPostStateSorted {
    /// Builds a sorted hashed post-state from reverts.
    ///
    /// - Reads the first occurrence of each changed account/storage slot in the range.
    /// - Hashes keys and returns them already ordered for trie iteration.
    #[instrument(target = "trie::db", skip(tx), fields(range))]
    fn from_reverts<KH: KeyHasher>(
        tx: &TX,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Self, DatabaseError> {
        let (accounts, storages) = read_reverts_from_changesets(tx, range)?;

        // Hash and sort accounts
        let mut hashed_accounts: Vec<_> =
            accounts.into_iter().map(|(address, info)| (KH::hash_key(address), info)).collect();
        hashed_accounts.sort_unstable_by_key(|(address, _)| *address);

        // Hash and sort storages
        let hashed_storages: B256Map<HashedStorageSorted> = storages
            .into_iter()
            .map(|(address, storage)| {
                let mut slots: Vec<_> =
                    storage.into_iter().map(|(slot, value)| (KH::hash_key(slot), value)).collect();
                slots.sort_unstable_by_key(|(slot, _)| *slot);
                (KH::hash_key(address), HashedStorageSorted { storage_slots: slots, wiped: false })
            })
            .collect();

        Ok(Self::new(hashed_accounts, hashed_storages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, map::HashMap, Address, B256, U256};
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::{
        database::Database,
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_trie::{HashedPostState, HashedStorage, KeccakKeyHasher};
    use revm::state::AccountInfo;
    use revm_database::BundleState;

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
        let overlay_root = StateRoot::overlay_root(&tx, &sorted).unwrap();

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
            StateRoot::overlay_root(&tx, &post_state.into_sorted()).unwrap(),
            hex!("b464525710cafcf5d4044ac85b72c08b1e76231b8d91f288fe438cc41d8eaafd")
        );
    }

    /// Verifies `from_reverts` keeps first occurrence per key and preserves ordering guarantees.
    #[test]
    fn from_reverts_keeps_first_occurrence_and_ordering() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().expect("failed to create rw tx");

        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let slot1 = B256::from(U256::from(11));
        let slot2 = B256::from(U256::from(22));

        // Account changesets: only first occurrence per address should be kept.
        tx.put::<tables::AccountChangeSets>(
            1,
            AccountBeforeTx {
                address: address1,
                info: Some(Account { nonce: 1, ..Default::default() }),
            },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(
            2,
            AccountBeforeTx {
                address: address1,
                info: Some(Account { nonce: 2, ..Default::default() }),
            },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(3, AccountBeforeTx { address: address2, info: None })
            .unwrap();

        // Storage changesets: only first occurrence per slot should be kept, and slots sorted.
        tx.put::<tables::StorageChangeSets>(
            BlockNumberAddress((1, address1)),
            StorageEntry { key: slot2, value: U256::from(200) },
        )
        .unwrap();
        tx.put::<tables::StorageChangeSets>(
            BlockNumberAddress((2, address1)),
            StorageEntry { key: slot1, value: U256::from(100) },
        )
        .unwrap();
        tx.put::<tables::StorageChangeSets>(
            BlockNumberAddress((3, address1)),
            StorageEntry { key: slot1, value: U256::from(999) }, // should be ignored
        )
        .unwrap();

        tx.commit().unwrap();
        let tx = db.tx().expect("failed to create ro tx");

        let sorted = HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, 1..=3).unwrap();

        // Verify first occurrences were kept (nonce 1, not 2)
        assert_eq!(sorted.accounts.len(), 2);
        let hashed_addr1 = KeccakKeyHasher::hash_key(address1);
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
        let db = create_test_rw_db();

        // Insert data outside the query range
        db.update(|tx| {
            tx.put::<tables::AccountChangeSets>(
                100,
                AccountBeforeTx {
                    address: Address::with_last_byte(1),
                    info: Some(Account { nonce: 1, ..Default::default() }),
                },
            )
            .unwrap();
        })
        .unwrap();

        let tx = db.tx().unwrap();

        // Query a range with no data
        let sorted = HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, 1..=10).unwrap();
        assert!(sorted.accounts.is_empty());
        assert!(sorted.storages.is_empty());
    }
}
