use core::ops::Not;

use crate::{
    added_removed_keys::MultiAddedRemovedKeys,
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    utils::extend_sorted_vec,
    KeyHasher, MultiProofTargets, Nibbles,
};
use alloc::{borrow::Cow, vec::Vec};
use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map, HashMap, HashSet},
    Address, B256, U256,
};
use itertools::Itertools;
#[cfg(feature = "rayon")]
pub use rayon::*;
use reth_primitives_traits::Account;

#[cfg(feature = "rayon")]
use rayon::prelude::{FromParallelIterator, IntoParallelIterator, ParallelIterator};

use revm_database::{AccountStatus, BundleAccount};

/// In-memory hashed state that stores account and storage changes with keccak256-hashed keys in
/// hash maps.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HashedPostState {
    /// Mapping of hashed address to account info, `None` if destroyed.
    pub accounts: B256Map<Option<Account>>,
    /// Mapping of hashed address to hashed storage.
    pub storages: B256Map<HashedStorage>,
}

impl HashedPostState {
    /// Create new instance of [`HashedPostState`].
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            accounts: B256Map::with_capacity_and_hasher(capacity, Default::default()),
            storages: B256Map::with_capacity_and_hasher(capacity, Default::default()),
        }
    }

    /// Initialize [`HashedPostState`] from bundle state.
    /// Hashes all changed accounts and storage entries that are currently stored in the bundle
    /// state.
    #[inline]
    #[cfg(feature = "rayon")]
    pub fn from_bundle_state<'a, KH: KeyHasher>(
        state: impl IntoParallelIterator<Item = (&'a Address, &'a BundleAccount)>,
    ) -> Self {
        state
            .into_par_iter()
            .map(|(address, account)| {
                let hashed_address = KH::hash_key(address);
                let hashed_account = account.info.as_ref().map(Into::into);
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                );

                (
                    hashed_address,
                    hashed_account,
                    (!hashed_storage.is_empty()).then_some(hashed_storage),
                )
            })
            .collect()
    }

    /// Initialize [`HashedPostState`] from bundle state.
    /// Hashes all changed accounts and storage entries that are currently stored in the bundle
    /// state.
    #[cfg(not(feature = "rayon"))]
    pub fn from_bundle_state<'a, KH: KeyHasher>(
        state: impl IntoIterator<Item = (&'a Address, &'a BundleAccount)>,
    ) -> Self {
        state
            .into_iter()
            .map(|(address, account)| {
                let hashed_address = KH::hash_key(address);
                let hashed_account = account.info.as_ref().map(Into::into);
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                );

                (
                    hashed_address,
                    hashed_account,
                    (!hashed_storage.is_empty()).then_some(hashed_storage),
                )
            })
            .collect()
    }

    /// Construct [`HashedPostState`] from a single [`HashedStorage`].
    pub fn from_hashed_storage(hashed_address: B256, storage: HashedStorage) -> Self {
        Self {
            accounts: HashMap::default(),
            storages: HashMap::from_iter([(hashed_address, storage)]),
        }
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

    /// Returns `true` if the hashed state is empty.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.storages.is_empty()
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
        let mut storage_prefix_sets =
            HashMap::with_capacity_and_hasher(self.storages.len(), Default::default());
        for (hashed_address, hashed_storage) in &self.storages {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            storage_prefix_sets.insert(*hashed_address, hashed_storage.construct_prefix_set());
        }

        TriePrefixSetsMut { account_prefix_set, storage_prefix_sets, destroyed_accounts }
    }

    /// Create multiproof targets for this state.
    pub fn multi_proof_targets(&self) -> MultiProofTargets {
        // Pre-allocate minimum capacity for the targets.
        let mut targets = MultiProofTargets::with_capacity(self.accounts.len());
        for hashed_address in self.accounts.keys() {
            targets.insert(*hashed_address, Default::default());
        }
        for (hashed_address, storage) in &self.storages {
            targets.entry(*hashed_address).or_default().extend(storage.storage.keys().copied());
        }
        targets
    }

    /// Create multiproof targets difference for this state,
    /// i.e., the targets that are in targets create from `self` but not in `excluded`.
    ///
    /// This method is preferred to first calling `Self::multi_proof_targets` and the calling
    /// `MultiProofTargets::retain_difference`, because it does not over allocate the targets map.
    pub fn multi_proof_targets_difference(
        &self,
        excluded: &MultiProofTargets,
    ) -> MultiProofTargets {
        let mut targets = MultiProofTargets::default();
        for hashed_address in self.accounts.keys() {
            if !excluded.contains_key(hashed_address) {
                targets.insert(*hashed_address, Default::default());
            }
        }
        for (hashed_address, storage) in &self.storages {
            let maybe_excluded_storage = excluded.get(hashed_address);
            let mut hashed_slots_targets = storage
                .storage
                .keys()
                .filter(|slot| !maybe_excluded_storage.is_some_and(|f| f.contains(*slot)))
                .peekable();
            if hashed_slots_targets.peek().is_some() {
                targets.entry(*hashed_address).or_default().extend(hashed_slots_targets);
            }
        }
        targets
    }

    /// Partition the state update into two state updates:
    /// - First with accounts and storages slots that are present in the provided targets.
    /// - Second with all other.
    ///
    /// CAUTION: The state updates are expected to be applied in order, so that the storage wipes
    /// are done correctly.
    pub fn partition_by_targets(
        mut self,
        targets: &MultiProofTargets,
        added_removed_keys: &MultiAddedRemovedKeys,
    ) -> (Self, Self) {
        let mut state_updates_not_in_targets = Self::default();

        self.storages.retain(|&address, storage| {
            let storage_added_removed_keys = added_removed_keys.get_storage(&address);

            let (retain, storage_not_in_targets) = match targets.get(&address) {
                Some(storage_in_targets) => {
                    let mut storage_not_in_targets = HashedStorage::default();
                    storage.storage.retain(|&slot, value| {
                        if storage_in_targets.contains(&slot) &&
                            !storage_added_removed_keys.is_some_and(|k| k.is_removed(&slot))
                        {
                            return true
                        }

                        storage_not_in_targets.storage.insert(slot, *value);
                        false
                    });

                    // We do not check the wiped flag here, because targets only contain addresses
                    // and storage slots. So if there are no storage slots left, the storage update
                    // can be fully removed.
                    let retain = !storage.storage.is_empty();

                    // Since state updates are expected to be applied in order, we can only set the
                    // wiped flag in the second storage update if the first storage update is empty
                    // and will not be retained.
                    if !retain {
                        storage_not_in_targets.wiped = storage.wiped;
                    }

                    (
                        retain,
                        storage_not_in_targets.is_empty().not().then_some(storage_not_in_targets),
                    )
                }
                None => (false, Some(core::mem::take(storage))),
            };

            if let Some(storage_not_in_targets) = storage_not_in_targets {
                state_updates_not_in_targets.storages.insert(address, storage_not_in_targets);
            }

            retain
        });
        self.accounts.retain(|&address, account| {
            if targets.contains_key(&address) {
                return true
            }

            state_updates_not_in_targets.accounts.insert(address, *account);
            false
        });

        (self, state_updates_not_in_targets)
    }

    /// Returns an iterator that yields chunks of the specified size.
    ///
    /// See [`ChunkedHashedPostState`] for more information.
    pub fn chunks(self, size: usize) -> ChunkedHashedPostState {
        ChunkedHashedPostState::new(self, size)
    }

    /// Returns the number of items that will be considered during chunking in `[Self::chunks]`.
    pub fn chunking_length(&self) -> usize {
        self.accounts.len() +
            self.storages
                .values()
                .map(|storage| if storage.wiped { 1 } else { 0 } + storage.storage.len())
                .sum::<usize>()
    }

    /// Extend this hashed post state with contents of another.
    /// Entries in the second hashed post state take precedence.
    pub fn extend(&mut self, other: Self) {
        self.extend_inner(Cow::Owned(other));
    }

    /// Extend this hashed post state with contents of another.
    /// Entries in the second hashed post state take precedence.
    ///
    /// Slightly less efficient than [`Self::extend`], but preferred to `extend(other.clone())`.
    pub fn extend_ref(&mut self, other: &Self) {
        self.extend_inner(Cow::Borrowed(other));
    }

    fn extend_inner(&mut self, other: Cow<'_, Self>) {
        self.accounts.extend(other.accounts.iter().map(|(&k, &v)| (k, v)));

        self.storages.reserve(other.storages.len());
        match other {
            Cow::Borrowed(other) => {
                self.extend_storages(other.storages.iter().map(|(k, v)| (*k, Cow::Borrowed(v))))
            }
            Cow::Owned(other) => {
                self.extend_storages(other.storages.into_iter().map(|(k, v)| (k, Cow::Owned(v))))
            }
        }
    }

    fn extend_storages<'a>(
        &mut self,
        storages: impl IntoIterator<Item = (B256, Cow<'a, HashedStorage>)>,
    ) {
        for (hashed_address, storage) in storages {
            match self.storages.entry(hashed_address) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(storage.into_owned());
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().extend(&storage);
                }
            }
        }
    }

    /// Extend this hashed post state with sorted data, converting directly into the unsorted
    /// `HashMap` representation. This is more efficient than first converting to `HashedPostState`
    /// and then extending, as it avoids creating intermediate `HashMap` allocations.
    pub fn extend_from_sorted(&mut self, sorted: &HashedPostStateSorted) {
        // Reserve capacity for accounts
        self.accounts.reserve(sorted.accounts.len());

        // Insert accounts (Some = updated, None = destroyed)
        for (address, account) in &sorted.accounts {
            self.accounts.insert(*address, *account);
        }

        // Reserve capacity for storages
        self.storages.reserve(sorted.storages.len());

        // Extend storages
        for (hashed_address, sorted_storage) in &sorted.storages {
            match self.storages.entry(*hashed_address) {
                hash_map::Entry::Vacant(entry) => {
                    let mut new_storage = HashedStorage::new(false);
                    new_storage.extend_from_sorted(sorted_storage);
                    entry.insert(new_storage);
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().extend_from_sorted(sorted_storage);
                }
            }
        }
    }

    /// Converts hashed post state into [`HashedPostStateSorted`].
    pub fn into_sorted(self) -> HashedPostStateSorted {
        let mut accounts: Vec<_> = self.accounts.into_iter().collect();
        accounts.sort_unstable_by_key(|(address, _)| *address);

        let storages = self
            .storages
            .into_iter()
            .map(|(hashed_address, storage)| (hashed_address, storage.into_sorted()))
            .collect();

        HashedPostStateSorted { accounts, storages }
    }

    /// Creates a sorted copy without consuming self.
    /// More efficient than `.clone().into_sorted()` as it avoids cloning `HashMap` metadata.
    pub fn clone_into_sorted(&self) -> HashedPostStateSorted {
        let mut accounts: Vec<_> = self.accounts.iter().map(|(&k, &v)| (k, v)).collect();
        accounts.sort_unstable_by_key(|(address, _)| *address);

        let storages = self
            .storages
            .iter()
            .map(|(&hashed_address, storage)| (hashed_address, storage.clone_into_sorted()))
            .collect();

        HashedPostStateSorted { accounts, storages }
    }

    /// Clears the account and storage maps of this `HashedPostState`.
    pub fn clear(&mut self) {
        self.accounts.clear();
        self.storages.clear();
    }
}

impl FromIterator<(B256, Option<Account>, Option<HashedStorage>)> for HashedPostState {
    /// Constructs a [`HashedPostState`] from an iterator of tuples containing:
    /// - Hashed address (B256)
    /// - Optional account info (`None` indicates destroyed account)
    /// - Optional hashed storage
    ///
    /// # Important
    ///
    /// - The iterator **assumes unique hashed addresses** (B256). If duplicate addresses are
    ///   present, later entries will overwrite earlier ones for accounts, and storage will be
    ///   merged.
    /// - The [`HashedStorage`] **must not be empty** (as determined by
    ///   [`HashedStorage::is_empty`]). Empty storage should be represented as `None` rather than
    ///   `Some(empty_storage)`. This ensures the storage map only contains meaningful entries.
    ///
    /// Use `(!storage.is_empty()).then_some(storage)` to convert empty storage to `None`.
    fn from_iter<T: IntoIterator<Item = (B256, Option<Account>, Option<HashedStorage>)>>(
        iter: T,
    ) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut hashed_state = Self::with_capacity(lower);

        for (hashed_address, info, hashed_storage) in iter {
            hashed_state.accounts.insert(hashed_address, info);
            if let Some(storage) = hashed_storage {
                hashed_state.storages.insert(hashed_address, storage);
            }
        }

        hashed_state
    }
}

#[cfg(feature = "rayon")]
impl FromParallelIterator<(B256, Option<Account>, Option<HashedStorage>)> for HashedPostState {
    /// Parallel version of [`FromIterator`] for constructing [`HashedPostState`] from a parallel
    /// iterator.
    ///
    /// See [`FromIterator::from_iter`] for details on the expected input format.
    ///
    /// # Important
    ///
    /// - The iterator **assumes unique hashed addresses** (B256). If duplicate addresses are
    ///   present, later entries will overwrite earlier ones for accounts, and storage will be
    ///   merged.
    /// - The [`HashedStorage`] **must not be empty**. Empty storage should be `None`.
    fn from_par_iter<I>(par_iter: I) -> Self
    where
        I: IntoParallelIterator<Item = (B256, Option<Account>, Option<HashedStorage>)>,
    {
        let vec: Vec<_> = par_iter.into_par_iter().collect();
        vec.into_iter().collect()
    }
}

/// Representation of in-memory hashed storage.
#[derive(PartialEq, Eq, Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HashedStorage {
    /// Flag indicating whether the storage was wiped or not.
    pub wiped: bool,
    /// Mapping of hashed storage slot to storage value.
    pub storage: B256Map<U256>,
}

impl HashedStorage {
    /// Create new instance of [`HashedStorage`].
    pub fn new(wiped: bool) -> Self {
        Self { wiped, storage: HashMap::default() }
    }

    /// Check if self is empty.
    pub fn is_empty(&self) -> bool {
        !self.wiped && self.storage.is_empty()
    }

    /// Create new hashed storage from iterator.
    pub fn from_iter(wiped: bool, iter: impl IntoIterator<Item = (B256, U256)>) -> Self {
        Self { wiped, storage: HashMap::from_iter(iter) }
    }

    /// Create new hashed storage from account status and plain storage.
    pub fn from_plain_storage<'a>(
        status: AccountStatus,
        storage: impl IntoIterator<Item = (&'a U256, &'a U256)>,
    ) -> Self {
        Self::from_iter(
            status.was_destroyed(),
            storage.into_iter().map(|(key, value)| (keccak256(B256::from(*key)), *value)),
        )
    }

    /// Construct [`PrefixSetMut`] from hashed storage.
    pub fn construct_prefix_set(&self) -> PrefixSetMut {
        if self.wiped {
            PrefixSetMut::all()
        } else {
            let mut prefix_set = PrefixSetMut::with_capacity(self.storage.len());
            for hashed_slot in self.storage.keys() {
                prefix_set.insert(Nibbles::unpack(hashed_slot));
            }
            prefix_set
        }
    }

    /// Extend hashed storage with contents of other.
    /// The entries in second hashed storage take precedence.
    pub fn extend(&mut self, other: &Self) {
        if other.wiped {
            self.wiped = true;
            self.storage.clear();
        }
        self.storage.extend(other.storage.iter().map(|(&k, &v)| (k, v)));
    }

    /// Extend hashed storage with sorted data, converting directly into the unsorted `HashMap`
    /// representation. This is more efficient than first converting to `HashedStorage` and
    /// then extending, as it avoids creating intermediate `HashMap` allocations.
    pub fn extend_from_sorted(&mut self, sorted: &HashedStorageSorted) {
        if sorted.wiped {
            self.wiped = true;
            self.storage.clear();
        }

        // Reserve capacity for all slots
        self.storage.reserve(sorted.storage_slots.len());

        // Insert all storage slots
        for (slot, value) in &sorted.storage_slots {
            self.storage.insert(*slot, *value);
        }
    }

    /// Converts hashed storage into [`HashedStorageSorted`].
    pub fn into_sorted(self) -> HashedStorageSorted {
        let mut storage_slots: Vec<_> = self.storage.into_iter().collect();
        storage_slots.sort_unstable_by_key(|(key, _)| *key);

        HashedStorageSorted { storage_slots, wiped: self.wiped }
    }

    /// Creates a sorted copy without consuming self.
    /// More efficient than `.clone().into_sorted()` as it avoids cloning `HashMap` metadata.
    pub fn clone_into_sorted(&self) -> HashedStorageSorted {
        let mut storage_slots: Vec<_> = self.storage.iter().map(|(&k, &v)| (k, v)).collect();
        storage_slots.sort_unstable_by_key(|(key, _)| *key);

        HashedStorageSorted { storage_slots, wiped: self.wiped }
    }
}

/// Sorted hashed post state optimized for iterating during state trie calculation.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HashedPostStateSorted {
    /// Sorted collection of account updates. `None` indicates a destroyed account.
    pub accounts: Vec<(B256, Option<Account>)>,
    /// Map of hashed addresses to their sorted storage updates.
    pub storages: B256Map<HashedStorageSorted>,
}

impl HashedPostStateSorted {
    /// Create new instance of [`HashedPostStateSorted`]
    pub const fn new(
        accounts: Vec<(B256, Option<Account>)>,
        storages: B256Map<HashedStorageSorted>,
    ) -> Self {
        Self { accounts, storages }
    }

    /// Returns reference to hashed accounts.
    pub const fn accounts(&self) -> &Vec<(B256, Option<Account>)> {
        &self.accounts
    }

    /// Returns reference to hashed account storages.
    pub const fn account_storages(&self) -> &B256Map<HashedStorageSorted> {
        &self.storages
    }

    /// Returns `true` if there are no account or storage updates.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.storages.is_empty()
    }

    /// Returns the total number of updates including all accounts and storage updates.
    pub fn total_len(&self) -> usize {
        self.accounts.len() + self.storages.values().map(|s| s.len()).sum::<usize>()
    }

    /// Construct [`TriePrefixSetsMut`] from hashed post state.
    ///
    /// The prefix sets contain the hashed account and storage keys that have been changed in the
    /// post state.
    pub fn construct_prefix_sets(&self) -> TriePrefixSetsMut {
        let mut account_prefix_set = PrefixSetMut::with_capacity(self.accounts.len());
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, account) in &self.accounts {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            if account.is_none() {
                destroyed_accounts.insert(*hashed_address);
            }
        }

        let mut storage_prefix_sets =
            B256Map::with_capacity_and_hasher(self.storages.len(), Default::default());
        for (hashed_address, hashed_storage) in &self.storages {
            // Ensure account trie covers storage overlays even if account map is empty.
            account_prefix_set.insert(Nibbles::unpack(hashed_address));

            let prefix_set = if hashed_storage.wiped {
                PrefixSetMut::all()
            } else {
                let mut prefix_set =
                    PrefixSetMut::with_capacity(hashed_storage.storage_slots.len());
                prefix_set.extend_keys(
                    hashed_storage
                        .storage_slots
                        .iter()
                        .map(|(hashed_slot, _)| Nibbles::unpack(hashed_slot)),
                );
                prefix_set
            };

            storage_prefix_sets.insert(*hashed_address, prefix_set);
        }

        TriePrefixSetsMut { account_prefix_set, storage_prefix_sets, destroyed_accounts }
    }

    /// Extends this state with contents of another sorted state.
    /// Entries in `other` take precedence for duplicate keys.
    ///
    /// Sorts the accounts after extending. Sorts the storage after extending, for each account.
    pub fn extend_ref_and_sort(&mut self, other: &Self) {
        // Extend accounts
        extend_sorted_vec(&mut self.accounts, &other.accounts);

        // Extend storages
        for (hashed_address, other_storage) in &other.storages {
            self.storages
                .entry(*hashed_address)
                .and_modify(|existing| existing.extend_ref(other_storage))
                .or_insert_with(|| other_storage.clone());
        }
    }

    /// Clears all accounts and storage data.
    pub fn clear(&mut self) {
        self.accounts.clear();
        self.storages.clear();
    }
}

impl AsRef<Self> for HashedPostStateSorted {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// Sorted hashed storage optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HashedStorageSorted {
    /// Sorted collection of updated storage slots. [`U256::ZERO`] indicates a deleted value.
    pub storage_slots: Vec<(B256, U256)>,
    /// Flag indicating whether the storage was wiped or not.
    pub wiped: bool,
}

impl HashedStorageSorted {
    /// Returns `true` if the account was wiped.
    pub const fn is_wiped(&self) -> bool {
        self.wiped
    }

    /// Returns reference to updated storage slots.
    pub fn storage_slots_ref(&self) -> &[(B256, U256)] {
        &self.storage_slots
    }

    /// Returns the total number of storage slot updates.
    pub const fn len(&self) -> usize {
        self.storage_slots.len()
    }

    /// Returns `true` if there are no storage slot updates.
    pub const fn is_empty(&self) -> bool {
        self.storage_slots.is_empty()
    }

    /// Extends the storage slots updates with another set of sorted updates.
    ///
    /// If `other` is marked as deleted, this will be marked as deleted and all slots cleared.
    /// Otherwise, nodes are merged with `other`'s values taking precedence for duplicates.
    pub fn extend_ref(&mut self, other: &Self) {
        if other.wiped {
            // If other is wiped, clear everything and copy from other
            self.wiped = true;
            self.storage_slots.clear();
            self.storage_slots.extend(other.storage_slots.iter().copied());
            return;
        }

        // Extend the sorted non-zero valued slots
        extend_sorted_vec(&mut self.storage_slots, &other.storage_slots);
    }
}

impl From<HashedStorageSorted> for HashedStorage {
    fn from(sorted: HashedStorageSorted) -> Self {
        let mut storage = B256Map::default();

        // Add all storage slots (including zero-valued ones which indicate deletion)
        for (slot, value) in sorted.storage_slots {
            storage.insert(slot, value);
        }

        Self { wiped: sorted.wiped, storage }
    }
}

impl From<HashedPostStateSorted> for HashedPostState {
    fn from(sorted: HashedPostStateSorted) -> Self {
        let mut accounts =
            B256Map::with_capacity_and_hasher(sorted.accounts.len(), Default::default());

        // Add all accounts (Some for updated, None for destroyed)
        for (address, account) in sorted.accounts {
            accounts.insert(address, account);
        }

        // Convert storages
        let storages = sorted
            .storages
            .into_iter()
            .map(|(address, storage)| (address, storage.into()))
            .collect();

        Self { accounts, storages }
    }
}

/// An iterator that yields chunks of the state updates of at most `size` account and storage
/// targets.
///
/// # Notes
/// 1. Chunks are expected to be applied in order, because of storage wipes. If applied out of
///    order, it's possible to wipe more storage than in the original state update.
/// 2. For each account, chunks with storage updates come first, followed by account updates.
#[derive(Debug)]
pub struct ChunkedHashedPostState {
    flattened: alloc::vec::IntoIter<(B256, FlattenedHashedPostStateItem)>,
    size: usize,
}

/// Order discriminant for sorting flattened state items.
/// Ordering: `StorageWipe` < `StorageUpdate` (by slot) < `Account`
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FlattenedStateOrder {
    StorageWipe,
    StorageUpdate(B256),
    Account,
}

#[derive(Debug)]
enum FlattenedHashedPostStateItem {
    Account(Option<Account>),
    StorageWipe,
    StorageUpdate { slot: B256, value: U256 },
}

impl FlattenedHashedPostStateItem {
    const fn order(&self) -> FlattenedStateOrder {
        match self {
            Self::StorageWipe => FlattenedStateOrder::StorageWipe,
            Self::StorageUpdate { slot, .. } => FlattenedStateOrder::StorageUpdate(*slot),
            Self::Account(_) => FlattenedStateOrder::Account,
        }
    }
}

impl ChunkedHashedPostState {
    fn new(hashed_post_state: HashedPostState, size: usize) -> Self {
        let flattened = hashed_post_state
            .storages
            .into_iter()
            .flat_map(|(address, storage)| {
                storage
                    .wiped
                    .then_some((address, FlattenedHashedPostStateItem::StorageWipe))
                    .into_iter()
                    .chain(storage.storage.into_iter().map(move |(slot, value)| {
                        (address, FlattenedHashedPostStateItem::StorageUpdate { slot, value })
                    }))
            })
            .chain(hashed_post_state.accounts.into_iter().map(|(address, account)| {
                (address, FlattenedHashedPostStateItem::Account(account))
            }))
            // Sort by address, then by item order to ensure correct application sequence:
            // 1. Storage wipes (must come first to clear storage)
            // 2. Storage updates (sorted by slot for determinism)
            // 3. Account updates (can be applied last)
            .sorted_unstable_by_key(|(address, item)| (*address, item.order()));

        Self { flattened, size }
    }
}

impl Iterator for ChunkedHashedPostState {
    type Item = HashedPostState;

    fn next(&mut self) -> Option<Self::Item> {
        let mut chunk = HashedPostState::default();

        let mut current_size = 0;
        while current_size < self.size {
            let Some((address, item)) = self.flattened.next() else { break };

            match item {
                FlattenedHashedPostStateItem::Account(account) => {
                    chunk.accounts.insert(address, account);
                }
                FlattenedHashedPostStateItem::StorageWipe => {
                    chunk.storages.entry(address).or_default().wiped = true;
                }
                FlattenedHashedPostStateItem::StorageUpdate { slot, value } => {
                    chunk.storages.entry(address).or_default().storage.insert(slot, value);
                }
            }

            current_size += 1;
        }

        if chunk.is_empty() {
            None
        } else {
            Some(chunk)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeccakKeyHasher;
    use alloy_primitives::Bytes;
    use revm_database::{states::StorageSlot, StorageWithOriginalValues};
    use revm_state::{AccountInfo, Bytecode};

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

    #[test]
    fn test_hashed_post_state_from_bundle_state() {
        // Prepare a random Ethereum address as a key for the account.
        let address = Address::random();

        // Create a mock account info object.
        let account_info = AccountInfo {
            balance: U256::from(123),
            nonce: 42,
            code_hash: B256::random(),
            code: Some(Bytecode::new_raw(Bytes::from(vec![1, 2]))),
        };

        let mut storage = StorageWithOriginalValues::default();
        storage.insert(
            U256::from(1),
            StorageSlot { present_value: U256::from(4), ..Default::default() },
        );

        // Create a `BundleAccount` struct to represent the account and its storage.
        let account = BundleAccount {
            status: AccountStatus::Changed,
            info: Some(account_info.clone()),
            storage,
            original_info: None,
        };

        // Create a vector of tuples representing the bundle state.
        let state = vec![(&address, &account)];

        // Convert the bundle state into a hashed post state.
        let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(state);

        // Validate the hashed post state.
        assert_eq!(hashed_state.accounts.len(), 1);
        assert_eq!(hashed_state.storages.len(), 1);

        // Validate the account info.
        assert_eq!(
            *hashed_state.accounts.get(&keccak256(address)).unwrap(),
            Some(account_info.into())
        );
    }

    #[test]
    fn test_hashed_post_state_with_accounts() {
        // Prepare random addresses and mock account info.
        let address_1 = Address::random();
        let address_2 = Address::random();

        let account_info_1 = AccountInfo {
            balance: U256::from(1000),
            nonce: 1,
            code_hash: B256::random(),
            code: None,
        };

        // Create hashed accounts with addresses.
        let account_1 = (keccak256(address_1), Some(account_info_1.into()));
        let account_2 = (keccak256(address_2), None);

        // Add accounts to the hashed post state.
        let hashed_state = HashedPostState::default().with_accounts(vec![account_1, account_2]);

        // Validate the hashed post state.
        assert_eq!(hashed_state.accounts.len(), 2);
        assert!(hashed_state.accounts.contains_key(&keccak256(address_1)));
        assert!(hashed_state.accounts.contains_key(&keccak256(address_2)));
    }

    #[test]
    fn test_hashed_post_state_with_storages() {
        // Prepare random addresses and mock storage entries.
        let address_1 = Address::random();
        let address_2 = Address::random();

        let storage_1 = (keccak256(address_1), HashedStorage::new(false));
        let storage_2 = (keccak256(address_2), HashedStorage::new(true));

        // Add storages to the hashed post state.
        let hashed_state = HashedPostState::default().with_storages(vec![storage_1, storage_2]);

        // Validate the hashed post state.
        assert_eq!(hashed_state.storages.len(), 2);
        assert!(hashed_state.storages.contains_key(&keccak256(address_1)));
        assert!(hashed_state.storages.contains_key(&keccak256(address_2)));
    }

    #[test]
    fn test_hashed_post_state_is_empty() {
        // Create an empty hashed post state and validate it's empty.
        let empty_state = HashedPostState::default();
        assert!(empty_state.is_empty());

        // Add an account and validate the state is no longer empty.
        let non_empty_state = HashedPostState::default()
            .with_accounts(vec![(keccak256(Address::random()), Some(Account::default()))]);
        assert!(!non_empty_state.is_empty());
    }

    fn create_state_for_multi_proof_targets() -> HashedPostState {
        let mut state = HashedPostState::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        let slot1 = B256::random();
        let slot2 = B256::random();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        state
    }

    #[test]
    fn test_multi_proof_targets_difference_empty_state() {
        let state = HashedPostState::default();
        let excluded = MultiProofTargets::default();

        let targets = state.multi_proof_targets_difference(&excluded);
        assert!(targets.is_empty());
    }

    #[test]
    fn test_multi_proof_targets_difference_new_account_targets() {
        let state = create_state_for_multi_proof_targets();
        let excluded = MultiProofTargets::default();

        // should return all accounts as targets since excluded is empty
        let targets = state.multi_proof_targets_difference(&excluded);
        assert_eq!(targets.len(), state.accounts.len());
        for addr in state.accounts.keys() {
            assert!(targets.contains_key(addr));
        }
    }

    #[test]
    fn test_multi_proof_targets_difference_new_storage_targets() {
        let state = create_state_for_multi_proof_targets();
        let excluded = MultiProofTargets::default();

        let targets = state.multi_proof_targets_difference(&excluded);

        // verify storage slots are included for accounts with storage
        for (addr, storage) in &state.storages {
            assert!(targets.contains_key(addr));
            let target_slots = &targets[addr];
            assert_eq!(target_slots.len(), storage.storage.len());
            for slot in storage.storage.keys() {
                assert!(target_slots.contains(slot));
            }
        }
    }

    #[test]
    fn test_multi_proof_targets_difference_filter_excluded_accounts() {
        let state = create_state_for_multi_proof_targets();
        let mut excluded = MultiProofTargets::default();

        // select an account that has no storage updates
        let excluded_addr = state
            .accounts
            .keys()
            .find(|&&addr| !state.storages.contains_key(&addr))
            .expect("Should have an account without storage");

        // mark the account as excluded
        excluded.insert(*excluded_addr, HashSet::default());

        let targets = state.multi_proof_targets_difference(&excluded);

        // should not include the already excluded account since it has no storage updates
        assert!(!targets.contains_key(excluded_addr));
        // other accounts should still be included
        assert_eq!(targets.len(), state.accounts.len() - 1);
    }

    #[test]
    fn test_multi_proof_targets_difference_filter_excluded_storage() {
        let state = create_state_for_multi_proof_targets();
        let mut excluded = MultiProofTargets::default();

        // mark one storage slot as excluded
        let (addr, storage) = state.storages.iter().next().unwrap();
        let mut excluded_slots = HashSet::default();
        let excluded_slot = *storage.storage.keys().next().unwrap();
        excluded_slots.insert(excluded_slot);
        excluded.insert(*addr, excluded_slots);

        let targets = state.multi_proof_targets_difference(&excluded);

        // should not include the excluded storage slot
        let target_slots = &targets[addr];
        assert!(!target_slots.contains(&excluded_slot));
        assert_eq!(target_slots.len(), storage.storage.len() - 1);
    }

    #[test]
    fn test_multi_proof_targets_difference_mixed_excluded_state() {
        let mut state = HashedPostState::default();
        let mut excluded = MultiProofTargets::default();

        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        state.accounts.insert(addr1, Some(Default::default()));
        state.accounts.insert(addr2, Some(Default::default()));

        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(1));
        state.storages.insert(addr1, storage);

        let mut excluded_slots = HashSet::default();
        excluded_slots.insert(slot1);
        excluded.insert(addr1, excluded_slots);

        let targets = state.multi_proof_targets_difference(&excluded);

        assert!(targets.contains_key(&addr2));
        assert!(!targets[&addr1].contains(&slot1));
        assert!(targets[&addr1].contains(&slot2));
    }

    #[test]
    fn test_multi_proof_targets_difference_unmodified_account_with_storage() {
        let mut state = HashedPostState::default();
        let excluded = MultiProofTargets::default();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // don't add the account to state.accounts (simulating unmodified account)
        // but add storage updates for this account
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::from(1));
        storage.storage.insert(slot2, U256::from(2));
        state.storages.insert(addr, storage);

        assert!(!state.accounts.contains_key(&addr));
        assert!(!excluded.contains_key(&addr));

        let targets = state.multi_proof_targets_difference(&excluded);

        // verify that we still get the storage slots for the unmodified account
        assert!(targets.contains_key(&addr));

        let target_slots = &targets[&addr];
        assert_eq!(target_slots.len(), 2);
        assert!(target_slots.contains(&slot1));
        assert!(target_slots.contains(&slot2));
    }

    #[test]
    fn test_partition_by_targets() {
        let addr1 = B256::random();
        let addr2 = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        let state = HashedPostState {
            accounts: B256Map::from_iter([
                (addr1, Some(Default::default())),
                (addr2, Some(Default::default())),
            ]),
            storages: B256Map::from_iter([(
                addr1,
                HashedStorage {
                    wiped: true,
                    storage: B256Map::from_iter([(slot1, U256::ZERO), (slot2, U256::from(1))]),
                },
            )]),
        };
        let targets = MultiProofTargets::from_iter([(addr1, HashSet::from_iter([slot1]))]);

        let (with_targets, without_targets) =
            state.partition_by_targets(&targets, &MultiAddedRemovedKeys::new());

        assert_eq!(
            with_targets,
            HashedPostState {
                accounts: B256Map::from_iter([(addr1, Some(Default::default()))]),
                storages: B256Map::from_iter([(
                    addr1,
                    HashedStorage {
                        wiped: true,
                        storage: B256Map::from_iter([(slot1, U256::ZERO)])
                    }
                )]),
            }
        );
        assert_eq!(
            without_targets,
            HashedPostState {
                accounts: B256Map::from_iter([(addr2, Some(Default::default()))]),
                storages: B256Map::from_iter([(
                    addr1,
                    HashedStorage {
                        wiped: false,
                        storage: B256Map::from_iter([(slot2, U256::from(1))])
                    }
                )]),
            }
        );
    }

    #[test]
    fn test_chunks() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let slot1 = B256::from([1; 32]);
        let slot2 = B256::from([2; 32]);

        let state = HashedPostState {
            accounts: B256Map::from_iter([
                (addr1, Some(Default::default())),
                (addr2, Some(Default::default())),
            ]),
            storages: B256Map::from_iter([(
                addr2,
                HashedStorage {
                    wiped: true,
                    storage: B256Map::from_iter([(slot1, U256::ZERO), (slot2, U256::from(1))]),
                },
            )]),
        };

        let mut chunks = state.chunks(2);
        assert_eq!(
            chunks.next(),
            Some(HashedPostState {
                accounts: B256Map::from_iter([(addr1, Some(Default::default()))]),
                storages: B256Map::from_iter([(addr2, HashedStorage::new(true)),])
            })
        );
        assert_eq!(
            chunks.next(),
            Some(HashedPostState {
                accounts: B256Map::default(),
                storages: B256Map::from_iter([(
                    addr2,
                    HashedStorage {
                        wiped: false,
                        storage: B256Map::from_iter([(slot1, U256::ZERO), (slot2, U256::from(1))]),
                    },
                )])
            })
        );
        assert_eq!(
            chunks.next(),
            Some(HashedPostState {
                accounts: B256Map::from_iter([(addr2, Some(Default::default()))]),
                storages: B256Map::default()
            })
        );
        assert_eq!(chunks.next(), None);
    }

    #[test]
    fn test_chunks_ordering_guarantee() {
        // Test that chunks preserve the ordering: wipe -> storage updates -> account
        // Use chunk size of 1 to verify each item comes out in the correct order
        let addr = B256::from([1; 32]);
        let slot1 = B256::from([1; 32]);
        let slot2 = B256::from([2; 32]);

        let state = HashedPostState {
            accounts: B256Map::from_iter([(addr, Some(Default::default()))]),
            storages: B256Map::from_iter([(
                addr,
                HashedStorage {
                    wiped: true,
                    storage: B256Map::from_iter([(slot1, U256::from(1)), (slot2, U256::from(2))]),
                },
            )]),
        };

        let chunks: Vec<_> = state.chunks(1).collect();

        // Should have 4 chunks: 1 wipe + 2 storage updates + 1 account
        assert_eq!(chunks.len(), 4);

        // First chunk must be the storage wipe
        assert!(chunks[0].accounts.is_empty());
        assert_eq!(chunks[0].storages.len(), 1);
        assert!(chunks[0].storages.get(&addr).unwrap().wiped);
        assert!(chunks[0].storages.get(&addr).unwrap().storage.is_empty());

        // Next two chunks must be storage updates (order between them doesn't matter)
        assert!(chunks[1].accounts.is_empty());
        assert!(!chunks[1].storages.get(&addr).unwrap().wiped);
        assert_eq!(chunks[1].storages.get(&addr).unwrap().storage.len(), 1);

        assert!(chunks[2].accounts.is_empty());
        assert!(!chunks[2].storages.get(&addr).unwrap().wiped);
        assert_eq!(chunks[2].storages.get(&addr).unwrap().storage.len(), 1);

        // Last chunk must be the account update
        assert_eq!(chunks[3].accounts.len(), 1);
        assert!(chunks[3].accounts.contains_key(&addr));
        assert!(chunks[3].storages.is_empty());
    }

    #[test]
    fn test_hashed_post_state_sorted_extend_ref() {
        // Test extending accounts
        let mut state1 = HashedPostStateSorted {
            accounts: vec![
                (B256::from([1; 32]), Some(Account::default())),
                (B256::from([3; 32]), Some(Account::default())),
                (B256::from([5; 32]), None),
            ],
            storages: B256Map::default(),
        };

        let state2 = HashedPostStateSorted {
            accounts: vec![
                (B256::from([2; 32]), Some(Account::default())),
                (B256::from([3; 32]), Some(Account { nonce: 1, ..Default::default() })), /* Override */
                (B256::from([4; 32]), Some(Account::default())),
                (B256::from([6; 32]), None),
            ],
            storages: B256Map::default(),
        };

        state1.extend_ref_and_sort(&state2);

        // Check accounts are merged and sorted
        assert_eq!(state1.accounts.len(), 6);
        assert_eq!(state1.accounts[0].0, B256::from([1; 32]));
        assert_eq!(state1.accounts[1].0, B256::from([2; 32]));
        assert_eq!(state1.accounts[2].0, B256::from([3; 32]));
        assert_eq!(state1.accounts[2].1.unwrap().nonce, 1); // Should have state2's value
        assert_eq!(state1.accounts[3].0, B256::from([4; 32]));
        assert_eq!(state1.accounts[4].0, B256::from([5; 32]));
        assert_eq!(state1.accounts[4].1, None);
        assert_eq!(state1.accounts[5].0, B256::from([6; 32]));
        assert_eq!(state1.accounts[5].1, None);
    }

    #[test]
    fn test_hashed_storage_sorted_extend_ref() {
        // Test normal extension
        let mut storage1 = HashedStorageSorted {
            storage_slots: vec![
                (B256::from([1; 32]), U256::from(10)),
                (B256::from([3; 32]), U256::from(30)),
                (B256::from([5; 32]), U256::ZERO),
            ],
            wiped: false,
        };

        let storage2 = HashedStorageSorted {
            storage_slots: vec![
                (B256::from([2; 32]), U256::from(20)),
                (B256::from([3; 32]), U256::from(300)), // Override
                (B256::from([4; 32]), U256::from(40)),
                (B256::from([6; 32]), U256::ZERO),
            ],
            wiped: false,
        };

        storage1.extend_ref(&storage2);

        assert_eq!(storage1.storage_slots.len(), 6);
        assert_eq!(storage1.storage_slots[0].0, B256::from([1; 32]));
        assert_eq!(storage1.storage_slots[0].1, U256::from(10));
        assert_eq!(storage1.storage_slots[1].0, B256::from([2; 32]));
        assert_eq!(storage1.storage_slots[1].1, U256::from(20));
        assert_eq!(storage1.storage_slots[2].0, B256::from([3; 32]));
        assert_eq!(storage1.storage_slots[2].1, U256::from(300)); // Should have storage2's value
        assert_eq!(storage1.storage_slots[3].0, B256::from([4; 32]));
        assert_eq!(storage1.storage_slots[3].1, U256::from(40));
        assert_eq!(storage1.storage_slots[4].0, B256::from([5; 32]));
        assert_eq!(storage1.storage_slots[4].1, U256::ZERO);
        assert_eq!(storage1.storage_slots[5].0, B256::from([6; 32]));
        assert_eq!(storage1.storage_slots[5].1, U256::ZERO);
        assert!(!storage1.wiped);

        // Test wiped storage
        let mut storage3 = HashedStorageSorted {
            storage_slots: vec![
                (B256::from([1; 32]), U256::from(10)),
                (B256::from([2; 32]), U256::ZERO),
            ],
            wiped: false,
        };

        let storage4 = HashedStorageSorted {
            storage_slots: vec![
                (B256::from([3; 32]), U256::from(30)),
                (B256::from([4; 32]), U256::ZERO),
            ],
            wiped: true,
        };

        storage3.extend_ref(&storage4);

        assert!(storage3.wiped);
        // When wiped, should only have storage4's values
        assert_eq!(storage3.storage_slots.len(), 2);
        assert_eq!(storage3.storage_slots[0].0, B256::from([3; 32]));
        assert_eq!(storage3.storage_slots[0].1, U256::from(30));
        assert_eq!(storage3.storage_slots[1].0, B256::from([4; 32]));
        assert_eq!(storage3.storage_slots[1].1, U256::ZERO);
    }

    /// Test extending with sorted accounts merges correctly into `HashMap`
    #[test]
    fn test_hashed_post_state_extend_from_sorted_with_accounts() {
        let addr1 = B256::random();
        let addr2 = B256::random();

        let mut state = HashedPostState::default();
        state.accounts.insert(addr1, Some(Default::default()));

        let mut sorted_state = HashedPostStateSorted::default();
        sorted_state.accounts.push((addr2, Some(Default::default())));

        state.extend_from_sorted(&sorted_state);

        assert_eq!(state.accounts.len(), 2);
        assert!(state.accounts.contains_key(&addr1));
        assert!(state.accounts.contains_key(&addr2));
    }

    /// Test destroyed accounts (None values) are inserted correctly
    #[test]
    fn test_hashed_post_state_extend_from_sorted_with_destroyed_accounts() {
        let addr1 = B256::random();

        let mut state = HashedPostState::default();

        let mut sorted_state = HashedPostStateSorted::default();
        sorted_state.accounts.push((addr1, None));

        state.extend_from_sorted(&sorted_state);

        assert!(state.accounts.contains_key(&addr1));
        assert_eq!(state.accounts.get(&addr1), Some(&None));
    }

    /// Test non-wiped storage merges both zero and non-zero valued slots
    #[test]
    fn test_hashed_storage_extend_from_sorted_non_wiped() {
        let slot1 = B256::random();
        let slot2 = B256::random();
        let slot3 = B256::random();

        let mut storage = HashedStorage::from_iter(false, [(slot1, U256::from(100))]);

        let sorted = HashedStorageSorted {
            storage_slots: vec![(slot2, U256::from(200)), (slot3, U256::ZERO)],
            wiped: false,
        };

        storage.extend_from_sorted(&sorted);

        assert!(!storage.wiped);
        assert_eq!(storage.storage.len(), 3);
        assert_eq!(storage.storage.get(&slot1), Some(&U256::from(100)));
        assert_eq!(storage.storage.get(&slot2), Some(&U256::from(200)));
        assert_eq!(storage.storage.get(&slot3), Some(&U256::ZERO));
    }

    /// Test wiped=true clears existing storage and only keeps new slots (critical edge case)
    #[test]
    fn test_hashed_storage_extend_from_sorted_wiped() {
        let slot1 = B256::random();
        let slot2 = B256::random();

        let mut storage = HashedStorage::from_iter(false, [(slot1, U256::from(100))]);

        let sorted =
            HashedStorageSorted { storage_slots: vec![(slot2, U256::from(200))], wiped: true };

        storage.extend_from_sorted(&sorted);

        assert!(storage.wiped);
        // After wipe, old storage should be cleared and only new storage remains
        assert_eq!(storage.storage.len(), 1);
        assert_eq!(storage.storage.get(&slot2), Some(&U256::from(200)));
    }

    #[test]
    fn test_hashed_post_state_chunking_length() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let addr3 = B256::from([3; 32]);
        let addr4 = B256::from([4; 32]);
        let slot1 = B256::from([1; 32]);
        let slot2 = B256::from([2; 32]);
        let slot3 = B256::from([3; 32]);

        let state = HashedPostState {
            accounts: B256Map::from_iter([(addr1, None), (addr2, None), (addr4, None)]),
            storages: B256Map::from_iter([
                (
                    addr1,
                    HashedStorage {
                        wiped: false,
                        storage: B256Map::from_iter([
                            (slot1, U256::ZERO),
                            (slot2, U256::ZERO),
                            (slot3, U256::ZERO),
                        ]),
                    },
                ),
                (
                    addr2,
                    HashedStorage {
                        wiped: true,
                        storage: B256Map::from_iter([
                            (slot1, U256::ZERO),
                            (slot2, U256::ZERO),
                            (slot3, U256::ZERO),
                        ]),
                    },
                ),
                (
                    addr3,
                    HashedStorage {
                        wiped: false,
                        storage: B256Map::from_iter([
                            (slot1, U256::ZERO),
                            (slot2, U256::ZERO),
                            (slot3, U256::ZERO),
                        ]),
                    },
                ),
            ]),
        };

        let chunking_length = state.chunking_length();
        for size in 1..=state.clone().chunks(1).count() {
            let chunk_count = state.clone().chunks(size).count();
            let expected_count = chunking_length.div_ceil(size);
            assert_eq!(
                chunk_count, expected_count,
                "chunking_length: {}, size: {}",
                chunking_length, size
            );
        }
    }

    #[test]
    fn test_clone_into_sorted_equivalence() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let addr3 = B256::from([3; 32]);
        let slot1 = B256::from([1; 32]);
        let slot2 = B256::from([2; 32]);
        let slot3 = B256::from([3; 32]);

        let state = HashedPostState {
            accounts: B256Map::from_iter([
                (addr1, Some(Account { nonce: 1, balance: U256::from(100), bytecode_hash: None })),
                (addr2, None),
                (addr3, Some(Account::default())),
            ]),
            storages: B256Map::from_iter([
                (
                    addr1,
                    HashedStorage {
                        wiped: false,
                        storage: B256Map::from_iter([
                            (slot1, U256::from(10)),
                            (slot2, U256::from(20)),
                        ]),
                    },
                ),
                (
                    addr2,
                    HashedStorage {
                        wiped: true,
                        storage: B256Map::from_iter([(slot3, U256::ZERO)]),
                    },
                ),
            ]),
        };

        // clone_into_sorted should produce the same result as clone().into_sorted()
        let sorted_via_clone = state.clone().into_sorted();
        let sorted_via_clone_into = state.clone_into_sorted();

        assert_eq!(sorted_via_clone, sorted_via_clone_into);

        // Verify the original state is not consumed
        assert_eq!(state.accounts.len(), 3);
        assert_eq!(state.storages.len(), 2);
    }

    #[test]
    fn test_hashed_storage_clone_into_sorted_equivalence() {
        let slot1 = B256::from([1; 32]);
        let slot2 = B256::from([2; 32]);
        let slot3 = B256::from([3; 32]);

        let storage = HashedStorage {
            wiped: true,
            storage: B256Map::from_iter([
                (slot1, U256::from(100)),
                (slot2, U256::ZERO),
                (slot3, U256::from(300)),
            ]),
        };

        // clone_into_sorted should produce the same result as clone().into_sorted()
        let sorted_via_clone = storage.clone().into_sorted();
        let sorted_via_clone_into = storage.clone_into_sorted();

        assert_eq!(sorted_via_clone, sorted_via_clone_into);

        // Verify the original storage is not consumed
        assert_eq!(storage.storage.len(), 3);
        assert!(storage.wiped);
    }
}

/// Bincode-compatible hashed state type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    use super::Account;
    use alloc::borrow::Cow;
    use alloy_primitives::{map::B256Map, B256, U256};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::HashedPostState`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, HashedPostState};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::hashed_state::HashedPostState")]
    ///     hashed_state: HashedPostState,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct HashedPostState<'a> {
        accounts: Cow<'a, B256Map<Option<Account>>>,
        storages: B256Map<HashedStorage<'a>>,
    }

    impl<'a> From<&'a super::HashedPostState> for HashedPostState<'a> {
        fn from(value: &'a super::HashedPostState) -> Self {
            Self {
                accounts: Cow::Borrowed(&value.accounts),
                storages: value.storages.iter().map(|(k, v)| (*k, v.into())).collect(),
            }
        }
    }

    impl<'a> From<HashedPostState<'a>> for super::HashedPostState {
        fn from(value: HashedPostState<'a>) -> Self {
            Self {
                accounts: value.accounts.into_owned(),
                storages: value.storages.into_iter().map(|(k, v)| (k, v.into())).collect(),
            }
        }
    }

    impl SerializeAs<super::HashedPostState> for HashedPostState<'_> {
        fn serialize_as<S>(
            source: &super::HashedPostState,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            HashedPostState::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::HashedPostState> for HashedPostState<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::HashedPostState, D::Error>
        where
            D: Deserializer<'de>,
        {
            HashedPostState::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::HashedStorage`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, HashedStorage};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::hashed_state::HashedStorage")]
    ///     hashed_storage: HashedStorage,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct HashedStorage<'a> {
        wiped: bool,
        storage: Cow<'a, B256Map<U256>>,
    }

    impl<'a> From<&'a super::HashedStorage> for HashedStorage<'a> {
        fn from(value: &'a super::HashedStorage) -> Self {
            Self { wiped: value.wiped, storage: Cow::Borrowed(&value.storage) }
        }
    }

    impl<'a> From<HashedStorage<'a>> for super::HashedStorage {
        fn from(value: HashedStorage<'a>) -> Self {
            Self { wiped: value.wiped, storage: value.storage.into_owned() }
        }
    }

    impl SerializeAs<super::HashedStorage> for HashedStorage<'_> {
        fn serialize_as<S>(source: &super::HashedStorage, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            HashedStorage::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::HashedStorage> for HashedStorage<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::HashedStorage, D::Error>
        where
            D: Deserializer<'de>,
        {
            HashedStorage::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::HashedPostStateSorted`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, HashedPostStateSorted};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::hashed_state::HashedPostStateSorted")]
    ///     hashed_state: HashedPostStateSorted,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct HashedPostStateSorted<'a> {
        accounts: Cow<'a, [(B256, Option<Account>)]>,
        storages: B256Map<HashedStorageSorted<'a>>,
    }

    impl<'a> From<&'a super::HashedPostStateSorted> for HashedPostStateSorted<'a> {
        fn from(value: &'a super::HashedPostStateSorted) -> Self {
            Self {
                accounts: Cow::Borrowed(&value.accounts),
                storages: value.storages.iter().map(|(k, v)| (*k, v.into())).collect(),
            }
        }
    }

    impl<'a> From<HashedPostStateSorted<'a>> for super::HashedPostStateSorted {
        fn from(value: HashedPostStateSorted<'a>) -> Self {
            Self {
                accounts: value.accounts.into_owned(),
                storages: value.storages.into_iter().map(|(k, v)| (k, v.into())).collect(),
            }
        }
    }

    impl SerializeAs<super::HashedPostStateSorted> for HashedPostStateSorted<'_> {
        fn serialize_as<S>(
            source: &super::HashedPostStateSorted,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            HashedPostStateSorted::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::HashedPostStateSorted> for HashedPostStateSorted<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::HashedPostStateSorted, D::Error>
        where
            D: Deserializer<'de>,
        {
            HashedPostStateSorted::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::HashedStorageSorted`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_trie_common::{serde_bincode_compat, HashedStorageSorted};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::hashed_state::HashedStorageSorted")]
    ///     hashed_storage: HashedStorageSorted,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct HashedStorageSorted<'a> {
        storage_slots: Cow<'a, [(B256, U256)]>,
        wiped: bool,
    }

    impl<'a> From<&'a super::HashedStorageSorted> for HashedStorageSorted<'a> {
        fn from(value: &'a super::HashedStorageSorted) -> Self {
            Self { storage_slots: Cow::Borrowed(&value.storage_slots), wiped: value.wiped }
        }
    }

    impl<'a> From<HashedStorageSorted<'a>> for super::HashedStorageSorted {
        fn from(value: HashedStorageSorted<'a>) -> Self {
            Self { storage_slots: value.storage_slots.into_owned(), wiped: value.wiped }
        }
    }

    impl SerializeAs<super::HashedStorageSorted> for HashedStorageSorted<'_> {
        fn serialize_as<S>(
            source: &super::HashedStorageSorted,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            HashedStorageSorted::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::HashedStorageSorted> for HashedStorageSorted<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::HashedStorageSorted, D::Error>
        where
            D: Deserializer<'de>,
        {
            HashedStorageSorted::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::{
            hashed_state::{
                HashedPostState, HashedPostStateSorted, HashedStorage, HashedStorageSorted,
            },
            serde_bincode_compat,
        };
        use alloy_primitives::{B256, U256};
        use reth_primitives_traits::Account;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_hashed_post_state_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::hashed_state::HashedPostState")]
                hashed_state: HashedPostState,
            }

            let mut data = Data { hashed_state: HashedPostState::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_state.accounts.insert(B256::random(), Some(Account::default()));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_state.storages.insert(B256::random(), HashedStorage::default());
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_hashed_storage_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::hashed_state::HashedStorage")]
                hashed_storage: HashedStorage,
            }

            let mut data = Data { hashed_storage: HashedStorage::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_storage.wiped = true;
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_storage.storage.insert(B256::random(), U256::from(1));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_hashed_post_state_sorted_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::hashed_state::HashedPostStateSorted")]
                hashed_state: HashedPostStateSorted,
            }

            let mut data = Data { hashed_state: HashedPostStateSorted::default() };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_state.accounts.push((B256::random(), Some(Account::default())));
            data.hashed_state
                .accounts
                .push((B256::random(), Some(Account { nonce: 1, ..Default::default() })));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_state.storages.insert(
                B256::random(),
                HashedStorageSorted {
                    storage_slots: vec![(B256::from([1; 32]), U256::from(10))],
                    wiped: false,
                },
            );
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_hashed_storage_sorted_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::hashed_state::HashedStorageSorted")]
                hashed_storage: HashedStorageSorted,
            }

            let mut data = Data {
                hashed_storage: HashedStorageSorted { storage_slots: Vec::new(), wiped: false },
            };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_storage.wiped = true;
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);

            data.hashed_storage.storage_slots.push((B256::random(), U256::from(1)));
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}
