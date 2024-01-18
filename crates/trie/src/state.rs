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

/// The post state with hashed addresses as keys.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HashedPostState {
    /// Collection of hashed addresses and their account info.
    pub(crate) accounts: Vec<(B256, Account)>,
    /// Set of destroyed account keys.
    pub(crate) destroyed_accounts: AHashSet<B256>,
    /// Map of hashed addresses to hashed storage.
    pub(crate) storages: AHashMap<B256, HashedStorage>,
    /// Flag indicating whether the account and storage entries were sorted.
    pub(crate) sorted: bool,
}

impl Default for HashedPostState {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            destroyed_accounts: AHashSet::new(),
            storages: AHashMap::new(),
            sorted: true, // empty is sorted
        }
    }
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
            this.insert_account(hashed_address, account.info.clone().map(into_reth_acc));

            // insert storage.
            let mut hashed_storage = HashedStorage::new(account.status.was_destroyed());

            for (key, value) in account.storage.iter() {
                let hashed_key = keccak256(B256::new(key.to_be_bytes()));
                hashed_storage.insert_slot(hashed_key, value.present_value);
            }
            this.insert_hashed_storage(hashed_address, hashed_storage)
        }
        this.sorted()
    }

    /// Initialize [HashedPostState] from revert range.
    /// Iterate over state reverts in the specified block range and
    /// apply them to hashed state in reverse.
    ///
    /// NOTE: In order to have the resulting [HashedPostState] be a correct
    /// overlay of the plain state, the end of the range must be the current tip.
    pub fn from_revert_range<TX: DbTx>(
        tx: TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, DatabaseError> {
        // A single map for aggregating state changes where each map value is a tuple
        // `(maybe_account_change, storage_changes)`.
        // If `maybe_account_change` is `None`, no account info change had occurred.
        // If `maybe_account_change` is `Some(None)`, the account had previously been destroyed
        // or non-existent.
        // If `maybe_account_change` is `Some(Some(info))`, the contained value is the previous
        // account state.
        let mut state =
            HashMap::<Address, (Option<Option<Account>>, HashMap<B256, U256>)>::default();

        // Iterate over account changesets in reverse.
        let mut account_changesets_cursor = tx.cursor_read::<tables::AccountChangeSet>()?;
        for entry in account_changesets_cursor.walk_range(range.clone())? {
            let (_, AccountBeforeTx { address, info }) = entry?;
            let account_entry = state.entry(address).or_default();
            if account_entry.0.is_none() {
                account_entry.0 = Some(info);
            }
        }

        // Iterate over storage changesets in reverse.
        let mut storage_changesets_cursor = tx.cursor_read::<tables::StorageChangeSet>()?;
        for entry in storage_changesets_cursor.walk_range(BlockNumberAddress::range(range))? {
            let (BlockNumberAddress((_, address)), storage) = entry?;
            let account_entry = state.entry(address).or_default();
            if let hash_map::Entry::Vacant(entry) = account_entry.1.entry(storage.key) {
                entry.insert(storage.value);
            }
        }

        let mut this = Self::default();
        for (address, (maybe_account_change, storage)) in state {
            let hashed_address = keccak256(address);

            if let Some(account_change) = maybe_account_change {
                this.insert_account(hashed_address, account_change);
            }

            // The `wiped`` flag indicates only  whether previous storage entries should be looked
            // up in db or not. For reverts it's a noop since all wiped changes had been written as
            // storage reverts.
            let mut hashed_storage = HashedStorage::new(false);
            for (slot, value) in storage {
                hashed_storage.insert_slot(keccak256(slot), value);
            }
            this.insert_hashed_storage(hashed_address, hashed_storage);
        }

        Ok(this.sorted())
    }

    /// Sort and return self.
    pub fn sorted(mut self) -> Self {
        self.sort();
        self
    }

    /// Returns all accounts with their state.
    pub fn accounts(&self) -> impl Iterator<Item = (B256, Option<Account>)> + '_ {
        self.destroyed_accounts.iter().map(|hashed_address| (*hashed_address, None)).chain(
            self.accounts.iter().map(|(hashed_address, account)| (*hashed_address, Some(*account))),
        )
    }

    /// Returns all account storages.
    pub fn storages(&self) -> impl Iterator<Item = (&B256, &HashedStorage)> {
        self.storages.iter()
    }

    /// Sort account and storage entries.
    pub fn sort(&mut self) {
        if !self.sorted {
            for (_, storage) in self.storages.iter_mut() {
                storage.sort_storage();
            }

            self.accounts.sort_unstable_by_key(|(address, _)| *address);
            self.sorted = true;
        }
    }

    /// Insert account. If `account` is `None`, the account had previously been destroyed.
    pub fn insert_account(&mut self, hashed_address: B256, account: Option<Account>) {
        if let Some(account) = account {
            self.accounts.push((hashed_address, account));
            self.sorted = false;
        } else {
            self.destroyed_accounts.insert(hashed_address);
        }
    }

    /// Insert hashed storage entry.
    pub fn insert_hashed_storage(&mut self, hashed_address: B256, hashed_storage: HashedStorage) {
        self.sorted &= hashed_storage.sorted;
        self.storages.insert(hashed_address, hashed_storage);
    }

    /// Returns all destroyed accounts.
    pub fn destroyed_accounts(&self) -> AHashSet<B256> {
        self.destroyed_accounts.clone()
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
        for hashed_address in &self.destroyed_accounts {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
        }

        // Populate storage prefix sets.
        for (hashed_address, hashed_storage) in self.storages.iter() {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            let storage_prefix_set_entry = storage_prefix_set.entry(*hashed_address).or_default();
            for (hashed_slot, _) in &hashed_storage.non_zero_valued_storage {
                storage_prefix_set_entry.insert(Nibbles::unpack(hashed_slot));
            }
            for hashed_slot in &hashed_storage.zero_valued_slots {
                storage_prefix_set_entry.insert(Nibbles::unpack(hashed_slot));
            }
        }

        (
            account_prefix_set.freeze(),
            storage_prefix_set.into_iter().map(|(k, v)| (k, v.freeze())).collect(),
        )
    }

    /// Returns [StateRoot] calculator based on database and in-memory state.
    fn state_root_calculator<'a, TX: DbTx>(
        &self,
        tx: &'a TX,
    ) -> StateRoot<&'a TX, HashedPostStateCursorFactory<'a, '_, TX>> {
        let (account_prefix_set, storage_prefix_set) = self.construct_prefix_sets();
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(tx, self);
        StateRoot::from_tx(tx)
            .with_hashed_cursor_factory(hashed_cursor_factory)
            .with_changed_account_prefixes(account_prefix_set)
            .with_changed_storage_prefixes(storage_prefix_set)
            .with_destroyed_accounts(self.destroyed_accounts())
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
        self.state_root_calculator(tx).root()
    }

    /// Calculates the state root for this [HashedPostState] and returns it alongside trie updates.
    /// See [Self::state_root] for more info.
    pub fn state_root_with_updates<TX: DbTx>(
        &self,
        tx: &TX,
    ) -> Result<(B256, TrieUpdates), StateRootError> {
        self.state_root_calculator(tx).root_with_updates()
    }
}

/// The post state account storage with hashed slots.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HashedStorage {
    /// Hashed storage slots with non-zero.
    pub(crate) non_zero_valued_storage: Vec<(B256, U256)>,
    /// Slots that have been zero valued.
    pub(crate) zero_valued_slots: AHashSet<B256>,
    /// Whether the storage was wiped or not.
    pub(crate) wiped: bool,
    /// Whether the storage entries were sorted or not.
    pub(crate) sorted: bool,
}

impl HashedStorage {
    /// Create new instance of [HashedStorage].
    pub fn new(wiped: bool) -> Self {
        Self {
            non_zero_valued_storage: Vec::new(),
            zero_valued_slots: AHashSet::new(),
            wiped,
            sorted: true, // empty is sorted
        }
    }

    /// Returns `true` if the storage was wiped.
    pub fn wiped(&self) -> bool {
        self.wiped
    }

    /// Returns all storage slots.
    pub fn storage_slots(&self) -> impl Iterator<Item = (B256, U256)> + '_ {
        self.zero_valued_slots
            .iter()
            .map(|slot| (*slot, U256::ZERO))
            .chain(self.non_zero_valued_storage.iter().cloned())
    }

    /// Sorts the non zero value storage entries.
    pub fn sort_storage(&mut self) {
        if !self.sorted {
            self.non_zero_valued_storage.sort_unstable_by_key(|(slot, _)| *slot);
            self.sorted = true;
        }
    }

    /// Insert storage entry.
    #[inline]
    pub fn insert_slot(&mut self, slot: B256, value: U256) {
        if value.is_zero() {
            self.zero_valued_slots.insert(slot);
        } else {
            self.non_zero_valued_storage.push((slot, value));
            self.sorted = false;
        }
    }
}
