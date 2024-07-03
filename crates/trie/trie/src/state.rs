use crate::{
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    trie_cursor::{TrieRangeWalker, TrieRangeWalkerFactory},
    Nibbles,
};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_primitives::{keccak256, Account, Address, BlockNumber, B256, U256};
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
    /// Initialize [`HashedPostState`] from bundle state.
    /// Hashes all changed accounts and storage entries that are currently stored in the bundle
    /// state.
    pub fn from_bundle_state<'a>(
        state: impl IntoParallelIterator<Item = (&'a Address, &'a BundleAccount)>,
    ) -> Self {
        let hashed = state
            .into_par_iter()
            .map(|(address, account)| {
                let hashed_address = keccak256(address);
                let hashed_account = account.info.clone().map(Into::into);
                let hashed_storage = HashedStorage::from_iter(
                    account.status.was_destroyed(),
                    account.storage.iter().map(|(key, value)| {
                        (keccak256(B256::new(key.to_be_bytes())), value.present_value)
                    }),
                );
                (hashed_address, (hashed_account, hashed_storage))
            })
            .collect::<Vec<(B256, (Option<Account>, HashedStorage))>>();

        let mut accounts = HashMap::with_capacity(hashed.len());
        let mut storages = HashMap::with_capacity(hashed.len());
        for (address, (account, storage)) in hashed {
            accounts.insert(address, account);
            storages.insert(address, storage);
        }
        Self { accounts, storages }
    }

    /// Initialize [`HashedPostState`] from revert range.
    /// Iterate over state reverts in the specified block range and
    /// apply them to hashed state in reverse.
    ///
    /// NOTE: In order to have the resulting [`HashedPostState`] be a correct
    /// overlay of the plain state, the end of the range must be the current tip.
    pub fn from_revert_range<T: TrieRangeWalkerFactory>(
        tx: &T,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Self, T::Err> {
        // Iterate over account changesets and record value before first occurring account change.
        let mut accounts = HashMap::<Address, Option<Account>>::default();
        let mut account_changesets_cursor = tx.account_change_sets()?;
        for entry in account_changesets_cursor.walk_range(range.clone())? {
            let (address, info) = entry?;
            if let hash_map::Entry::Vacant(entry) = accounts.entry(address) {
                entry.insert(info);
            }
        }

        // Iterate over storage changesets and record value before first occurring storage change.
        let mut storages = HashMap::<Address, HashMap<B256, U256>>::default();
        let mut storage_changesets_cursor = tx.storage_change_sets()?;
        for entry in storage_changesets_cursor.walk_range(range)? {
            let (address, key, value) = entry?;
            let account_storage = storages.entry(address).or_default();
            if let hash_map::Entry::Vacant(entry) = account_storage.entry(key) {
                entry.insert(value);
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

    /// Converts hashed post state into [`HashedPostStateSorted`].
    pub fn into_sorted(self) -> HashedPostStateSorted {
        let mut updated_accounts = Vec::new();
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, info) in self.accounts {
            if let Some(info) = info {
                updated_accounts.push((hashed_address, info));
            } else {
                destroyed_accounts.insert(hashed_address);
            }
        }
        updated_accounts.sort_unstable_by_key(|(address, _)| *address);
        let accounts = HashedAccountsSorted { accounts: updated_accounts, destroyed_accounts };

        let storages = self
            .storages
            .into_iter()
            .map(|(hashed_address, storage)| (hashed_address, storage.into_sorted()))
            .collect();

        HashedPostStateSorted { accounts, storages }
    }

    /// Construct [`TriePrefixSetsMut`] from hashed post state.
    /// The prefix sets contain the hashed account and storage keys that have been changed in the
    /// post state.
    pub fn construct_prefix_sets(&self) -> TriePrefixSetsMut {
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
        for (hashed_address, hashed_storage) in &self.storages {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            let mut prefix_set = PrefixSetMut::with_capacity(hashed_storage.storage.len());
            for hashed_slot in hashed_storage.storage.keys() {
                prefix_set.insert(Nibbles::unpack(hashed_slot));
            }
            storage_prefix_sets.insert(*hashed_address, prefix_set);
        }

        TriePrefixSetsMut { account_prefix_set, storage_prefix_sets, destroyed_accounts }
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
    /// Create new instance of [`HashedStorage`].
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

    /// Converts hashed storage into [`HashedStorageSorted`].
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
    /// Updated state of accounts.
    pub(crate) accounts: HashedAccountsSorted,
    /// Map of hashed addresses to hashed storage.
    pub(crate) storages: HashMap<B256, HashedStorageSorted>,
}

/// Sorted account state optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct HashedAccountsSorted {
    /// Sorted collection of hashed addresses and their account info.
    pub(crate) accounts: Vec<(B256, Account)>,
    /// Set of destroyed account keys.
    pub(crate) destroyed_accounts: HashSet<B256>,
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
