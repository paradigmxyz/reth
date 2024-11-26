use crate::{
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    Nibbles,
};
use alloy_primitives::{keccak256, Address, B256, U256};
use itertools::Itertools;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_primitives::Account;
use revm::db::{states::CacheAccount, AccountStatus, BundleAccount};
use std::{
    borrow::Cow,
    collections::{hash_map, HashMap, HashSet},
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
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
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

    /// Initialize [`HashedPostState`] from cached state.
    /// Hashes all changed accounts and storage entries that are currently stored in cache.
    pub fn from_cache_state<'a>(
        state: impl IntoParallelIterator<Item = (&'a Address, &'a CacheAccount)>,
    ) -> Self {
        let hashed = state
            .into_par_iter()
            .map(|(address, account)| {
                let hashed_address = keccak256(address);
                let hashed_account = account.account.as_ref().map(|a| a.info.clone().into());
                let hashed_storage = HashedStorage::from_plain_storage(
                    account.status,
                    account.account.as_ref().map(|a| a.storage.iter()).into_iter().flatten(),
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

    /// Construct [`HashedPostState`] from a single [`HashedStorage`].
    pub fn from_hashed_storage(hashed_address: B256, storage: HashedStorage) -> Self {
        Self { accounts: HashMap::default(), storages: HashMap::from([(hashed_address, storage)]) }
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
        let mut storage_prefix_sets = HashMap::with_capacity(self.storages.len());
        for (hashed_address, hashed_storage) in &self.storages {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            storage_prefix_sets.insert(*hashed_address, hashed_storage.construct_prefix_set());
        }

        TriePrefixSetsMut { account_prefix_set, storage_prefix_sets, destroyed_accounts }
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
    pub(crate) accounts: HashedAccountsSorted,
    /// Map of hashed addresses to hashed storage.
    pub(crate) storages: HashMap<B256, HashedStorageSorted>,
}

impl HashedPostStateSorted {
    /// Create new instance of [`HashedPostStateSorted`]
    pub const fn new(
        accounts: HashedAccountsSorted,
        storages: HashMap<B256, HashedStorageSorted>,
    ) -> Self {
        Self { accounts, storages }
    }

    /// Returns reference to hashed accounts.
    pub const fn accounts(&self) -> &HashedAccountsSorted {
        &self.accounts
    }

    /// Returns reference to hashed account storages.
    pub const fn account_storages(&self) -> &HashMap<B256, HashedStorageSorted> {
        &self.storages
    }
}

/// Sorted account state optimized for iterating during state trie calculation.
#[derive(Clone, Eq, PartialEq, Default, Debug)]
pub struct HashedAccountsSorted {
    /// Sorted collection of hashed addresses and their account info.
    pub(crate) accounts: Vec<(B256, Account)>,
    /// Set of destroyed account keys.
    pub(crate) destroyed_accounts: HashSet<B256>,
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
    pub(crate) non_zero_valued_slots: Vec<(B256, U256)>,
    /// Slots that have been zero valued.
    pub(crate) zero_valued_slots: HashSet<B256>,
    /// Flag indicating whether the storage was wiped or not.
    pub(crate) wiped: bool,
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use revm::{
        db::{
            states::{plain_account::PlainStorage, StorageSlot},
            PlainAccount, StorageWithOriginalValues,
        },
        primitives::{AccountInfo, Bytecode},
    };

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
            code: Some(Bytecode::LegacyRaw(Bytes::from(vec![1, 2]))),
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
        let hashed_state = HashedPostState::from_bundle_state(state);

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
    fn test_hashed_post_state_from_cache_state() {
        // Prepare a random Ethereum address.
        let address = Address::random();

        // Create mock account info.
        let account_info = AccountInfo {
            balance: U256::from(500),
            nonce: 5,
            code_hash: B256::random(),
            code: None,
        };

        let mut storage = PlainStorage::default();
        storage.insert(U256::from(1), U256::from(35636));

        // Create a `CacheAccount` with the mock account info.
        let account = CacheAccount {
            account: Some(PlainAccount { info: account_info.clone(), storage }),
            status: AccountStatus::Changed,
        };

        // Create a vector of tuples representing the cache state.
        let state = vec![(&address, &account)];

        // Convert the cache state into a hashed post state.
        let hashed_state = HashedPostState::from_cache_state(state);

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
}
