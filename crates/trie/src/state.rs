use crate::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::{PrefixSetMut, TriePrefixSets},
    updates::TrieUpdates,
    StateRoot, StateRootError,
};
use reth_db::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives::{
    keccak256, revm::compat::into_reth_acc, trie::Nibbles, Account, Address, BlockNumber, B256,
    U256,
};
use revm::db::BundleAccount;
use std::{
    collections::{hash_map, HashMap, HashSet},
    ops::RangeInclusive,
};

/// Representation of in-memory hashed state.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct HashedPostState {
    /// Mapping of hashed address to account info, `None` if destroyed.
    pub accounts: HashMap<B256, Option<Account>>,
    /// Mapping of hashed address to hashed storage.
    pub storages: HashMap<B256, HashedStorage>,
}

impl HashedPostState {
    /// Initialize [HashedPostState] from bundle state.
    /// Hashes all changed accounts and storage entries that are currently stored in the bundle
    /// state.
    pub fn from_bundle_state<'a>(
        state: impl IntoIterator<Item = (&'a Address, &'a BundleAccount)>,
    ) -> Self {
        let mut this = Self::default();
        for (address, account) in state {
            let hashed_address = keccak256(address);
            this.accounts.insert(hashed_address, account.info.clone().map(into_reth_acc));

            let hashed_storage = HashedStorage::from_iter(
                account.status.was_destroyed(),
                account.storage.iter().map(|(key, value)| {
                    (keccak256(B256::new(key.to_be_bytes())), value.present_value)
                }),
            );
            this.storages.insert(hashed_address, hashed_storage);
        }
        this
    }

    /// Initialize [HashedPostState] from revert range.
    /// Iterate over state reverts in the specified block range and
    /// apply them to hashed state in reverse.
    ///
    /// NOTE: In order to have the resulting [HashedPostState] be a correct
    /// overlay of the plain state, the end of the range must be the current tip.
    pub fn from_revert_range<TX: DbTx>(
        tx: &TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, DatabaseError> {
        // Iterate over account changesets and record value before first occurring account change.
        let mut accounts = HashMap::<Address, Option<Account>>::default();
        let mut account_changesets_cursor = tx.cursor_read::<tables::AccountChangeSet>()?;
        for entry in account_changesets_cursor.walk_range(range.clone())? {
            let (_, AccountBeforeTx { address, info }) = entry?;
            if let hash_map::Entry::Vacant(entry) = accounts.entry(address) {
                entry.insert(info);
            }
        }

        // Iterate over storage changesets and record value before first occurring storage change.
        let mut storages = HashMap::<Address, HashMap<B256, U256>>::default();
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSet>()?;
        for entry in storage_changesets_cursor.walk_range(BlockNumberAddress::range(range))? {
            let (BlockNumberAddress((_, address)), storage) = entry?;
            let account_storage = storages.entry(address).or_default();
            if let hash_map::Entry::Vacant(entry) = account_storage.entry(storage.key) {
                entry.insert(storage.value);
            }
        }

        let hashed_accounts = HashMap::from_iter(
            accounts.into_iter().map(|(address, info)| (keccak256(address), info)),
        );

        let hashed_storages = HashMap::from_iter(storages.into_iter().map(|(address, storage)| {
            (
                keccak256(address),
                HashedStorage::from_iter(
                    // The `wiped` flag indicates only whether previous storage entries
                    // should be looked up in db or not. For reverts it's a noop since all
                    // wiped changes had been written as storage reverts.
                    false,
                    storage.into_iter().map(|(slot, value)| (keccak256(slot), value)),
                ),
            )
        }));

        Ok(Self { accounts: hashed_accounts, storages: hashed_storages })
    }

    /// Set account entries on hashed state.
    pub fn with_accounts(
        mut self,
        accounts: impl IntoIterator<Item = (B256, Option<Account>)>,
    ) -> Self {
        self.accounts = HashMap::from_iter(accounts);
        self
    }

    /// Set storage entries on hashed state.
    pub fn with_storages(
        mut self,
        storages: impl IntoIterator<Item = (B256, HashedStorage)>,
    ) -> Self {
        self.storages = HashMap::from_iter(storages);
        self
    }

    /// Extend this hashed post state with contents of another.
    /// Entries in the second hashed post state take precedence.
    pub fn extend(&mut self, other: Self) {
        for (hashed_address, account) in other.accounts {
            self.accounts.insert(hashed_address, account);
        }

        for (hashed_address, storage) in other.storages {
            match self.storages.entry(hashed_address) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(storage);
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().extend(storage);
                }
            }
        }
    }

    /// Converts hashed post state into [HashedPostStateSorted].
    pub fn into_sorted(self) -> HashedPostStateSorted {
        let mut accounts = Vec::new();
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, info) in self.accounts {
            if let Some(info) = info {
                accounts.push((hashed_address, info));
            } else {
                destroyed_accounts.insert(hashed_address);
            }
        }
        accounts.sort_unstable_by_key(|(address, _)| *address);

        let storages = self
            .storages
            .into_iter()
            .map(|(hashed_address, storage)| (hashed_address, storage.into_sorted()))
            .collect();

        HashedPostStateSorted { accounts, destroyed_accounts, storages }
    }

    /// Construct [TriePrefixSets] from hashed post state.
    /// The prefix sets contain the hashed account and storage keys that have been changed in the
    /// post state.
    pub fn construct_prefix_sets(&self) -> TriePrefixSets {
        // Populate account prefix set.
        let mut account_prefix_set = PrefixSetMut::with_capacity(self.accounts.len());
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, account) in &self.accounts {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            if account.is_none() {
                destroyed_accounts.insert(*hashed_address);
            }
        }

        // Populate storage prefix sets.
        let mut storage_prefix_sets = HashMap::with_capacity(self.storages.len());
        for (hashed_address, hashed_storage) in self.storages.iter() {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            let mut prefix_set = PrefixSetMut::with_capacity(hashed_storage.storage.len());
            for hashed_slot in hashed_storage.storage.keys() {
                prefix_set.insert(Nibbles::unpack(hashed_slot));
            }
            storage_prefix_sets.insert(*hashed_address, prefix_set.freeze());
        }

        TriePrefixSets {
            account_prefix_set: account_prefix_set.freeze(),
            storage_prefix_sets,
            destroyed_accounts,
        }
    }

    /// Calculate the state root for this [HashedPostState].
    /// Internally, this method retrieves prefixsets and uses them
    /// to calculate incremental state root.
    ///
    /// # Example
    ///
    /// ```
    /// use reth_db::{database::Database, test_utils::create_test_rw_db};
    /// use reth_primitives::{Account, U256};
    /// use reth_trie::HashedPostState;
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
    /// let state_root = hashed_state.state_root(&tx);
    /// ```
    ///
    /// # Returns
    ///
    /// The state root for this [HashedPostState].
    pub fn state_root<TX: DbTx>(&self, tx: &TX) -> Result<B256, StateRootError> {
        let sorted = self.clone().into_sorted();
        let prefix_sets = self.construct_prefix_sets();
        StateRoot::from_tx(tx)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(tx, &sorted))
            .with_prefix_sets(prefix_sets)
            .root()
    }

    /// Calculates the state root for this [HashedPostState] and returns it alongside trie updates.
    /// See [Self::state_root] for more info.
    pub fn state_root_with_updates<TX: DbTx>(
        &self,
        tx: &TX,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let sorted = self.clone().into_sorted();
        let prefix_sets = self.construct_prefix_sets();
        StateRoot::from_tx(tx)
            .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(tx, &sorted))
            .with_prefix_sets(prefix_sets)
            .root_with_updates()
    }
}

/// Representation of in-memory hashed storage.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct HashedStorage {
    /// Flag indicating whether the storage was wiped or not.
    pub wiped: bool,
    /// Mapping of hashed storage slot to storage value.
    pub storage: HashMap<B256, U256>,
}

impl HashedStorage {
    /// Create new instance of [HashedStorage].
    pub fn new(wiped: bool) -> Self {
        Self { wiped, storage: HashMap::default() }
    }

    /// Create new hashed storage from iterator.
    pub fn from_iter(wiped: bool, iter: impl IntoIterator<Item = (B256, U256)>) -> Self {
        Self { wiped, storage: HashMap::from_iter(iter) }
    }

    /// Extend hashed storage with contents of other.
    /// The entries in second hashed storage take precedence.
    pub fn extend(&mut self, other: Self) {
        if other.wiped {
            self.wiped = true;
            self.storage.clear();
        }
        for (hashed_slot, value) in other.storage {
            self.storage.insert(hashed_slot, value);
        }
    }

    /// Converts hashed storage into [HashedStorageSorted].
    pub fn into_sorted(self) -> HashedStorageSorted {
        let mut non_zero_valued_slots = Vec::new();
        let mut zero_valued_slots = HashSet::default();
        for (hashed_slot, value) in self.storage {
            if value == U256::ZERO {
                zero_valued_slots.insert(hashed_slot);
            } else {
                non_zero_valued_slots.push((hashed_slot, value));
            }
        }
        non_zero_valued_slots.sort_unstable_by_key(|(key, _)| *key);

        HashedStorageSorted { non_zero_valued_slots, zero_valued_slots, wiped: self.wiped }
    }
}

/// Sorted hashed post state optimized for iterating during state trie calculation.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct HashedPostStateSorted {
    /// Sorted collection of hashed addresses and their account info.
    pub(crate) accounts: Vec<(B256, Account)>,
    /// Set of destroyed account keys.
    pub(crate) destroyed_accounts: HashSet<B256>,
    /// Map of hashed addresses to hashed storage.
    pub(crate) storages: HashMap<B256, HashedStorageSorted>,
}

/// Sorted hashed storage optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct HashedStorageSorted {
    /// Sorted hashed storage slots with non-zero value.
    pub(crate) non_zero_valued_slots: Vec<(B256, U256)>,
    /// Slots that have been zero valued.
    pub(crate) zero_valued_slots: HashSet<B256>,
    /// Flag indicating hether the storage was wiped or not.
    pub(crate) wiped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_state_wiped_extension() {
        let hashed_address = B256::default();
        let hashed_slot = B256::with_last_byte(64);
        let hashed_slot2 = B256::with_last_byte(65);

        // Initialize post state storage
        let original_slot_value = U256::from(123);
        let mut hashed_state = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(
                false,
                [(hashed_slot, original_slot_value), (hashed_slot2, original_slot_value)],
            ),
        )]);

        // Update single slot value
        let updated_slot_value = U256::from(321);
        let extension = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot, updated_slot_value)]),
        )]);
        hashed_state.extend(extension);

        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&updated_slot_value)
        );
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot2)),
            Some(&original_slot_value)
        );
        assert_eq!(account_storage.map(|st| st.wiped), Some(false));

        // Wipe account storage
        let wiped_extension =
            HashedPostState::default().with_storages([(hashed_address, HashedStorage::new(true))]);
        hashed_state.extend(wiped_extension);

        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(account_storage.map(|st| st.storage.is_empty()), Some(true));
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));

        // Reinitialize single slot value
        hashed_state.extend(HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot, original_slot_value)]),
        )]));
        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&original_slot_value)
        );
        assert_eq!(account_storage.and_then(|st| st.storage.get(&hashed_slot2)), None);
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));

        // Reinitialize single slot value
        hashed_state.extend(HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot2, updated_slot_value)]),
        )]));
        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&original_slot_value)
        );
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot2)),
            Some(&updated_slot_value)
        );
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));
    }
}
