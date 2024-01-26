use crate::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::{PrefixSet, PrefixSetMut},
    updates::TrieUpdates,
    StateRoot, StateRootError,
};
use ahash::{AHashMap, AHashSet};
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
    collections::{hash_map, HashMap},
    ops::RangeInclusive,
};

/// Representation of in-memory hashed state.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct HashedPostState {
    /// Mapping of hashed address to account info, `None` if destroyed.
    pub accounts: AHashMap<B256, Option<Account>>,
    /// Mapping of hashed address to hashed storage.
    pub storages: AHashMap<B256, HashedStorage>,
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
        let mut this = Self::default();

        // Iterate over account changesets and record value before first occurring account change.
        let mut account_changesets_cursor = tx.cursor_read::<tables::AccountChangeSet>()?;
        for entry in account_changesets_cursor.walk_range(range.clone())? {
            let (_, AccountBeforeTx { address, info }) = entry?;
            let hashed_address = keccak256(address); // TODO: cache hashes?
            if let hash_map::Entry::Vacant(entry) = this.accounts.entry(hashed_address) {
                entry.insert(info);
            }
        }

        // Iterate over storage changesets and record value before first occurring storage change.
        let mut storages = AHashMap::<Address, HashMap<B256, U256>>::default();
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSet>()?;
        for entry in storage_changesets_cursor.walk_range(BlockNumberAddress::range(range))? {
            let (BlockNumberAddress((_, address)), storage) = entry?;
            let account_storage = storages.entry(address).or_default();
            if let hash_map::Entry::Vacant(entry) = account_storage.entry(storage.key) {
                entry.insert(storage.value);
            }
        }

        for (address, storage) in storages {
            // The `wiped` flag indicates only whether previous storage entries should be looked
            // up in db or not. For reverts it's a noop since all wiped changes had been written as
            // storage reverts.
            let hashed_storage = HashedStorage::from_iter(
                false,
                storage.into_iter().map(|(slot, value)| (keccak256(slot), value)),
            );
            this.storages.insert(keccak256(address), hashed_storage);
        }

        Ok(this)
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
        let mut destroyed_accounts = AHashSet::default();
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

    /// Construct [PrefixSet] from hashed post state.
    /// The prefix sets contain the hashed account and storage keys that have been changed in the
    /// post state.
    pub fn construct_prefix_sets(&self) -> (PrefixSet, AHashMap<B256, PrefixSet>) {
        // Initialize prefix sets.
        let mut account_prefix_set = PrefixSetMut::default();
        let mut storage_prefix_set: AHashMap<B256, PrefixSetMut> = AHashMap::default();

        // Populate account prefix set.
        for (hashed_address, _) in &self.accounts {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
        }

        // Populate storage prefix sets.
        for (hashed_address, hashed_storage) in self.storages.iter() {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            let storage_prefix_set_entry = storage_prefix_set.entry(*hashed_address).or_default();
            for (hashed_slot, _) in &hashed_storage.storage {
                storage_prefix_set_entry.insert(Nibbles::unpack(hashed_slot));
            }
        }

        (
            account_prefix_set.freeze(),
            storage_prefix_set.into_iter().map(|(k, v)| (k, v.freeze())).collect(),
        )
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
    /// hashed_state.insert_account(
    ///     [0x11; 32].into(),
    ///     Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None }),
    /// );
    /// hashed_state.sort();
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
        let (account_prefix_set, storage_prefix_set) = self.construct_prefix_sets();
        sorted
            .state_root_calculator(tx)
            .with_changed_account_prefixes(account_prefix_set)
            .with_changed_storage_prefixes(storage_prefix_set)
            .root()
    }

    /// Calculates the state root for this [HashedPostState] and returns it alongside trie updates.
    /// See [Self::state_root] for more info.
    pub fn state_root_with_updates<TX: DbTx>(
        &self,
        tx: &TX,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        let sorted = self.clone().into_sorted();
        let (account_prefix_set, storage_prefix_set) = self.construct_prefix_sets();
        sorted
            .state_root_calculator(tx)
            .with_changed_account_prefixes(account_prefix_set)
            .with_changed_storage_prefixes(storage_prefix_set)
            .root_with_updates()
    }
}

/// Representation of in-memory hashed storage.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct HashedStorage {
    /// Flag indicating whether the storage was wiped or not.
    pub wiped: bool,
    /// Mapping of hashed storage slot to storage value.
    pub storage: AHashMap<B256, U256>,
}

impl HashedStorage {
    /// Create new instance of [HashedStorage].
    pub fn new(wiped: bool) -> Self {
        Self { wiped, storage: AHashMap::default() }
    }

    /// Create new hashed storage from iterator.
    pub fn from_iter(wiped: bool, iter: impl IntoIterator<Item = (B256, U256)>) -> Self {
        Self { wiped, storage: AHashMap::from_iter(iter) }
    }

    /// Extend hashed storage with contents of other.
    /// The entries in second hashed storage take precedence.
    pub fn extend(&mut self, other: Self) {
        for (hashed_slot, value) in other.storage {
            self.storage.insert(hashed_slot, value);
        }
        self.wiped |= other.wiped;
    }

    /// Converts hashed storage into [HashedStorageSorted].
    pub fn into_sorted(self) -> HashedStorageSorted {
        let mut non_zero_valued_slots = Vec::new();
        let mut zero_valued_slots = AHashSet::default();
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
    pub(crate) destroyed_accounts: AHashSet<B256>,
    /// Map of hashed addresses to hashed storage.
    pub(crate) storages: AHashMap<B256, HashedStorageSorted>,
}

impl HashedPostStateSorted {
    /// Returns all destroyed accounts.
    pub fn destroyed_accounts(&self) -> AHashSet<B256> {
        self.destroyed_accounts.clone()
    }

    /// Returns [StateRoot] calculator based on database and in-memory state.
    fn state_root_calculator<'a, TX: DbTx>(
        &self,
        tx: &'a TX,
    ) -> StateRoot<&'a TX, HashedPostStateCursorFactory<'_, &'a TX>> {
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(tx, self);
        StateRoot::from_tx(tx)
            .with_hashed_cursor_factory(hashed_cursor_factory)
            .with_destroyed_accounts(self.destroyed_accounts())
    }
}

/// Sorted hashed storage optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct HashedStorageSorted {
    /// Sorted hashed storage slots with non-zero value.
    pub(crate) non_zero_valued_slots: Vec<(B256, U256)>,
    /// Slots that have been zero valued.
    pub(crate) zero_valued_slots: AHashSet<B256>,
    /// Flag indicating hether the storage was wiped or not.
    pub(crate) wiped: bool,
}
