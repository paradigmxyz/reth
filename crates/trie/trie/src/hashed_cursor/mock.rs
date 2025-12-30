use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

use crate::mock::{KeyVisit, KeyVisitType};

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use alloy_primitives::{map::B256Map, B256, U256};
use parking_lot::{Mutex, MutexGuard};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostState;
use tracing::instrument;

/// Mock hashed cursor factory.
#[derive(Clone, Default, Debug)]
pub struct MockHashedCursorFactory {
    hashed_accounts: Arc<BTreeMap<B256, Account>>,
    hashed_storage_tries: Arc<B256Map<BTreeMap<B256, U256>>>,

    /// List of keys that the hashed accounts cursor has visited.
    visited_account_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>,
    /// List of keys that the hashed storages cursor has visited, per storage trie.
    visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<B256>>>>>,
}

impl MockHashedCursorFactory {
    /// Creates a new mock hashed cursor factory.
    pub fn new(
        hashed_accounts: BTreeMap<B256, Account>,
        hashed_storage_tries: B256Map<BTreeMap<B256, U256>>,
    ) -> Self {
        let visited_storage_keys =
            hashed_storage_tries.keys().map(|k| (*k, Default::default())).collect();
        Self {
            hashed_accounts: Arc::new(hashed_accounts),
            hashed_storage_tries: Arc::new(hashed_storage_tries),
            visited_account_keys: Default::default(),
            visited_storage_keys: Arc::new(visited_storage_keys),
        }
    }

    /// Creates a new mock hashed cursor factory from a `HashedPostState`.
    pub fn from_hashed_post_state(post_state: HashedPostState) -> Self {
        // Extract accounts from post state, filtering out None (deleted accounts)
        let hashed_accounts: BTreeMap<B256, Account> = post_state
            .accounts
            .into_iter()
            .filter_map(|(addr, account)| account.map(|acc| (addr, acc)))
            .collect();

        // Extract storages from post state
        let mut hashed_storages: B256Map<BTreeMap<B256, U256>> = post_state
            .storages
            .into_iter()
            .map(|(addr, hashed_storage)| {
                // Convert HashedStorage to BTreeMap, filtering out zero values (deletions)
                let storage_map: BTreeMap<B256, U256> = hashed_storage
                    .storage
                    .into_iter()
                    .filter_map(|(slot, value)| (value != U256::ZERO).then_some((slot, value)))
                    .collect();
                (addr, storage_map)
            })
            .collect();

        // Ensure all accounts have at least an empty storage
        for account in hashed_accounts.keys() {
            hashed_storages.entry(*account).or_default();
        }

        Self::new(hashed_accounts, hashed_storages)
    }

    /// Returns a reference to the list of visited hashed account keys.
    pub fn visited_account_keys(&self) -> MutexGuard<'_, Vec<KeyVisit<B256>>> {
        self.visited_account_keys.lock()
    }

    /// Returns a reference to the list of visited hashed storage keys for the given hashed address.
    pub fn visited_storage_keys(
        &self,
        hashed_address: B256,
    ) -> MutexGuard<'_, Vec<KeyVisit<B256>>> {
        self.visited_storage_keys.get(&hashed_address).expect("storage trie should exist").lock()
    }
}

impl HashedCursorFactory for MockHashedCursorFactory {
    type AccountCursor<'a>
        = MockHashedCursor<Account>
    where
        Self: 'a;
    type StorageCursor<'a>
        = MockHashedCursor<U256>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(MockHashedCursor::new(self.hashed_accounts.clone(), self.visited_account_keys.clone()))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        MockHashedCursor::new_storage(
            self.hashed_storage_tries.clone(),
            self.visited_storage_keys.clone(),
            hashed_address,
        )
    }
}

/// Mock hashed cursor type - determines whether this is an account or storage cursor.
#[derive(Debug)]
enum MockHashedCursorType<T> {
    Account {
        values: Arc<BTreeMap<B256, T>>,
        visited_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>,
    },
    Storage {
        all_storage_values: Arc<B256Map<BTreeMap<B256, T>>>,
        all_visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<B256>>>>>,
        current_hashed_address: B256,
    },
}

/// Mock hashed cursor.
#[derive(Debug)]
pub struct MockHashedCursor<T> {
    /// The current key. If set, it is guaranteed to exist in `values`.
    current_key: Option<B256>,
    cursor_type: MockHashedCursorType<T>,
}

impl<T> MockHashedCursor<T> {
    /// Creates a new mock hashed cursor for accounts with the given values and key tracking.
    pub const fn new(
        values: Arc<BTreeMap<B256, T>>,
        visited_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>,
    ) -> Self {
        Self {
            current_key: None,
            cursor_type: MockHashedCursorType::Account { values, visited_keys },
        }
    }

    /// Creates a new mock hashed cursor for storage with access to all storage tries.
    pub fn new_storage(
        all_storage_values: Arc<B256Map<BTreeMap<B256, T>>>,
        all_visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<B256>>>>>,
        hashed_address: B256,
    ) -> Result<Self, DatabaseError> {
        if !all_storage_values.contains_key(&hashed_address) {
            return Err(DatabaseError::Other(format!(
                "storage trie for {hashed_address:?} not found"
            )));
        }
        Ok(Self {
            current_key: None,
            cursor_type: MockHashedCursorType::Storage {
                all_storage_values,
                all_visited_storage_keys,
                current_hashed_address: hashed_address,
            },
        })
    }

    /// Returns the values map for the current cursor type.
    fn values(&self) -> &BTreeMap<B256, T> {
        match &self.cursor_type {
            MockHashedCursorType::Account { values, .. } => values.as_ref(),
            MockHashedCursorType::Storage {
                all_storage_values, current_hashed_address, ..
            } => all_storage_values
                .get(current_hashed_address)
                .expect("current_hashed_address should exist in all_storage_values"),
        }
    }

    /// Returns the visited keys mutex for the current cursor type.
    fn visited_keys(&self) -> &Mutex<Vec<KeyVisit<B256>>> {
        match &self.cursor_type {
            MockHashedCursorType::Account { visited_keys, .. } => visited_keys.as_ref(),
            MockHashedCursorType::Storage {
                all_visited_storage_keys,
                current_hashed_address,
                ..
            } => all_visited_storage_keys
                .get(current_hashed_address)
                .expect("current_hashed_address should exist in all_visited_storage_keys"),
        }
    }
}

impl<T: Debug + Clone> HashedCursor for MockHashedCursor<T> {
    type Value = T;

    #[instrument(skip(self), ret(level = "trace"))]
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Find the first key that is greater than or equal to the given key.
        let entry = self.values().iter().find_map(|(k, v)| (k >= &key).then(|| (*k, v.clone())));
        if let Some((key, _)) = &entry {
            self.current_key = Some(*key);
        }
        self.visited_keys().lock().push(KeyVisit {
            visit_type: KeyVisitType::SeekNonExact(key),
            visited_key: entry.as_ref().map(|(k, _)| *k),
        });
        Ok(entry)
    }

    #[instrument(skip(self), ret(level = "trace"))]
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let mut iter = self.values().iter();
        // Jump to the first key that has a prefix of the current key if it's set, or to the first
        // key otherwise.
        iter.find(|(k, _)| {
            self.current_key.as_ref().is_none_or(|current| k.starts_with(current.as_slice()))
        })
        .expect("current key should exist in values");
        // Get the next key-value pair.
        let entry = iter.next().map(|(k, v)| (*k, v.clone()));
        if let Some((key, _)) = &entry {
            self.current_key = Some(*key);
        }
        self.visited_keys().lock().push(KeyVisit {
            visit_type: KeyVisitType::Next,
            visited_key: entry.as_ref().map(|(k, _)| *k),
        });
        Ok(entry)
    }

    fn reset(&mut self) {
        self.current_key = None;
    }
}

impl<T: Debug + Clone> HashedStorageCursor for MockHashedCursor<T> {
    #[instrument(level = "trace", skip(self), ret)]
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(self.values().is_empty())
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        match &mut self.cursor_type {
            MockHashedCursorType::Storage { current_hashed_address, .. } => {
                *current_hashed_address = hashed_address;
            }
            MockHashedCursorType::Account { .. } => {
                panic!("set_hashed_address called on account cursor")
            }
        }
    }
}
