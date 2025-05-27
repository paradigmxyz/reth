use core::ops::Not;

use crate::{
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    KeyHasher, MultiProofTargets, Nibbles,
};
use alloc::{borrow::Cow, vec::Vec};
use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map, B256Set, HashMap, HashSet},
    Address, B256, U256,
};
use itertools::Itertools;
#[cfg(feature = "rayon")]
pub use rayon::*;
use reth_primitives_traits::Account;

#[cfg(feature = "rayon")]
use rayon::prelude::{IntoParallelIterator, ParallelIterator};

use revm_database::{AccountStatus, BundleAccount};

/// Representation of in-memory hashed state.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
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
        let hashed = state
            .into_par_iter()
            .map(|(address, account)| {
                let hashed_address = KH::hash_key(address);
                let hashed_account = account.info.as_ref().map(Into::into);
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                );
                (hashed_address, (hashed_account, hashed_storage))
            })
            .collect::<Vec<(B256, (Option<Account>, HashedStorage))>>();

        let mut accounts = HashMap::with_capacity_and_hasher(hashed.len(), Default::default());
        let mut storages = HashMap::with_capacity_and_hasher(hashed.len(), Default::default());
        for (address, (account, storage)) in hashed {
            accounts.insert(address, account);
            if !storage.is_empty() {
                storages.insert(address, storage);
            }
        }
        Self { accounts, storages }
    }

    /// Initialize [`HashedPostState`] from bundle state.
    /// Hashes all changed accounts and storage entries that are currently stored in the bundle
    /// state.
    #[cfg(not(feature = "rayon"))]
    pub fn from_bundle_state<'a, KH: KeyHasher>(
        state: impl IntoIterator<Item = (&'a Address, &'a BundleAccount)>,
    ) -> Self {
        let hashed = state
            .into_iter()
            .map(|(address, account)| {
                let hashed_address = KH::hash_key(address);
                let hashed_account = account.info.as_ref().map(Into::into);
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                );
                (hashed_address, (hashed_account, hashed_storage))
            })
            .collect::<Vec<(B256, (Option<Account>, HashedStorage))>>();

        let mut accounts = HashMap::with_capacity_and_hasher(hashed.len(), Default::default());
        let mut storages = HashMap::with_capacity_and_hasher(hashed.len(), Default::default());
        for (address, (account, storage)) in hashed {
            accounts.insert(address, account);
            if !storage.is_empty() {
                storages.insert(address, storage);
            }
        }
        Self { accounts, storages }
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
    pub fn partition_by_targets(mut self, targets: &MultiProofTargets) -> (Self, Self) {
        let mut state_updates_not_in_targets = Self::default();

        self.storages.retain(|&address, storage| {
            let (retain, storage_not_in_targets) = match targets.get(&address) {
                Some(storage_in_targets) => {
                    let mut storage_not_in_targets = HashedStorage::default();
                    storage.storage.retain(|&slot, value| {
                        if storage_in_targets.contains(&slot) {
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
}

/// Representation of in-memory hashed storage.
#[derive(PartialEq, Eq, Clone, Debug, Default)]
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

    /// Converts hashed storage into [`HashedStorageSorted`].
    pub fn into_sorted(self) -> HashedStorageSorted {
        let mut non_zero_valued_slots = Vec::new();
        let mut zero_valued_slots = HashSet::default();
        for (hashed_slot, value) in self.storage {
            if value.is_zero() {
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
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct HashedPostStateSorted {
    /// Updated state of accounts.
    pub accounts: HashedAccountsSorted,
    /// Map of hashed addresses to hashed storage.
    pub storages: B256Map<HashedStorageSorted>,
}

impl HashedPostStateSorted {
    /// Create new instance of [`HashedPostStateSorted`]
    pub const fn new(
        accounts: HashedAccountsSorted,
        storages: B256Map<HashedStorageSorted>,
    ) -> Self {
        Self { accounts, storages }
    }

    /// Returns reference to hashed accounts.
    pub const fn accounts(&self) -> &HashedAccountsSorted {
        &self.accounts
    }

    /// Returns reference to hashed account storages.
    pub const fn account_storages(&self) -> &B256Map<HashedStorageSorted> {
        &self.storages
    }
}

/// Sorted account state optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Default, Debug)]
pub struct HashedAccountsSorted {
    /// Sorted collection of hashed addresses and their account info.
    pub accounts: Vec<(B256, Account)>,
    /// Set of destroyed account keys.
    pub destroyed_accounts: B256Set,
}

impl HashedAccountsSorted {
    /// Returns a sorted iterator over updated accounts.
    pub fn accounts_sorted(&self) -> impl Iterator<Item = (B256, Option<Account>)> {
        self.accounts
            .iter()
            .map(|(address, account)| (*address, Some(*account)))
            .chain(self.destroyed_accounts.iter().map(|address| (*address, None)))
            .sorted_by_key(|entry| *entry.0)
    }
}

/// Sorted hashed storage optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct HashedStorageSorted {
    /// Sorted hashed storage slots with non-zero value.
    pub non_zero_valued_slots: Vec<(B256, U256)>,
    /// Slots that have been zero valued.
    pub zero_valued_slots: B256Set,
    /// Flag indicating whether the storage was wiped or not.
    pub wiped: bool,
}

impl HashedStorageSorted {
    /// Returns `true` if the account was wiped.
    pub const fn is_wiped(&self) -> bool {
        self.wiped
    }

    /// Returns a sorted iterator over updated storage slots.
    pub fn storage_slots_sorted(&self) -> impl Iterator<Item = (B256, U256)> {
        self.non_zero_valued_slots
            .iter()
            .map(|(hashed_slot, value)| (*hashed_slot, *value))
            .chain(self.zero_valued_slots.iter().map(|hashed_slot| (*hashed_slot, U256::ZERO)))
            .sorted_by_key(|entry| *entry.0)
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

#[derive(Debug)]
enum FlattenedHashedPostStateItem {
    Account(Option<Account>),
    StorageWipe,
    StorageUpdate { slot: B256, value: U256 },
}

impl ChunkedHashedPostState {
    fn new(hashed_post_state: HashedPostState, size: usize) -> Self {
        let flattened = hashed_post_state
            .storages
            .into_iter()
            .flat_map(|(address, storage)| {
                // Storage wipes should go first
                Some((address, FlattenedHashedPostStateItem::StorageWipe))
                    .filter(|_| storage.wiped)
                    .into_iter()
                    .chain(
                        storage.storage.into_iter().sorted_unstable_by_key(|(slot, _)| *slot).map(
                            move |(slot, value)| {
                                (
                                    address,
                                    FlattenedHashedPostStateItem::StorageUpdate { slot, value },
                                )
                            },
                        ),
                    )
            })
            .chain(hashed_post_state.accounts.into_iter().map(|(address, account)| {
                (address, FlattenedHashedPostStateItem::Account(account))
            }))
            // We need stable sort here to preserve the order for each address:
            // 1. Storage wipes
            // 2. Storage updates
            // 3. Account update
            .sorted_by_key(|(address, _)| *address);

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

        let (with_targets, without_targets) = state.partition_by_targets(&targets);

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
}
