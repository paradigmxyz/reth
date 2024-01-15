use crate::{
    hashed_cursor::HashedPostStateCursorFactory,
    prefix_set::{PrefixSet, PrefixSetMut},
    updates::TrieUpdates,
    StateRoot, StateRootError,
};
use ahash::{AHashMap, AHashSet};
use reth_db::transaction::DbTx;
use reth_primitives::{
    keccak256, revm::compat::into_reth_acc, trie::Nibbles, Account, Address, B256, U256,
};
use revm::db::BundleAccount;

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
        let mut hashed_state = Self::default();

        for (address, account) in state {
            let hashed_address = keccak256(address);
            if let Some(account) = &account.info {
                hashed_state.insert_account(hashed_address, into_reth_acc(account.clone()))
            } else {
                hashed_state.insert_destroyed_account(hashed_address);
            }

            // insert storage.
            let mut hashed_storage = HashedStorage::new(account.status.was_destroyed());

            for (key, value) in account.storage.iter() {
                let hashed_key = keccak256(B256::new(key.to_be_bytes()));
                hashed_storage.insert_storage(hashed_key, value.present_value);
            }
            hashed_state.insert_hashed_storage(hashed_address, hashed_storage)
        }
        hashed_state.sorted()
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

    /// Insert non-empty account info.
    pub fn insert_account(&mut self, hashed_address: B256, account: Account) {
        self.accounts.push((hashed_address, account));
        self.sorted = false;
    }

    /// Insert destroyed hashed account key.
    pub fn insert_destroyed_account(&mut self, hashed_address: B256) {
        self.destroyed_accounts.insert(hashed_address);
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
    ///     Account { nonce: 1, balance: U256::from(10), bytecode_hash: None },
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
    pub fn insert_storage(&mut self, slot: B256, value: U256) {
        if value.is_zero() {
            self.zero_valued_slots.insert(slot);
        } else {
            self.non_zero_valued_storage.push((slot, value));
            self.sorted = false;
        }
    }
}
