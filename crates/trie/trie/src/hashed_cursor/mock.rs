use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

use crate::mock::{KeyVisit, KeyVisitType};

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use alloy_primitives::{map::B256Map, B256, U256};
use parking_lot::{Mutex, MutexGuard};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use tracing::instrument;

/// Mock hashed cursor factory.
#[derive(Clone, Default, Debug)]
pub struct MockHashedCursorFactory {
    hashed_accounts: Arc<BTreeMap<B256, Account>>,
    hashed_storage_tries: B256Map<Arc<BTreeMap<B256, U256>>>,

    /// List of keys that the hashed accounts cursor has visited.
    visited_account_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>,
    /// List of keys that the hashed storages cursor has visited, per storage trie.
    visited_storage_keys: B256Map<Arc<Mutex<Vec<KeyVisit<B256>>>>>,
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
            hashed_storage_tries: hashed_storage_tries
                .into_iter()
                .map(|(k, v)| (k, Arc::new(v)))
                .collect(),
            visited_account_keys: Default::default(),
            visited_storage_keys,
        }
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
    type AccountCursor = MockHashedCursor<Account>;
    type StorageCursor = MockHashedCursor<U256>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(MockHashedCursor::new(self.hashed_accounts.clone(), self.visited_account_keys.clone()))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(MockHashedCursor::new(
            self.hashed_storage_tries
                .get(&hashed_address)
                .ok_or_else(|| {
                    DatabaseError::Other(format!("storage trie for {hashed_address:?} not found"))
                })?
                .clone(),
            self.visited_storage_keys
                .get(&hashed_address)
                .ok_or_else(|| {
                    DatabaseError::Other(format!("storage trie for {hashed_address:?} not found"))
                })?
                .clone(),
        ))
    }
}

/// Mock hashed cursor.
#[derive(Default, Debug)]
pub struct MockHashedCursor<T> {
    /// The current key. If set, it is guaranteed to exist in `values`.
    current_key: Option<B256>,
    values: Arc<BTreeMap<B256, T>>,
    visited_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>,
}

impl<T> MockHashedCursor<T> {
    fn new(values: Arc<BTreeMap<B256, T>>, visited_keys: Arc<Mutex<Vec<KeyVisit<B256>>>>) -> Self {
        Self { current_key: None, values, visited_keys }
    }
}

impl<T: Debug + Clone> HashedCursor for MockHashedCursor<T> {
    type Value = T;

    #[instrument(level = "trace", skip(self), ret)]
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Find the first key that is greater than or equal to the given key.
        let entry = self.values.iter().find_map(|(k, v)| (k >= &key).then(|| (*k, v.clone())));
        if let Some((key, _)) = &entry {
            self.current_key = Some(*key);
        }
        self.visited_keys.lock().push(KeyVisit {
            visit_type: KeyVisitType::SeekNonExact(key),
            visited_key: entry.as_ref().map(|(k, _)| *k),
        });
        Ok(entry)
    }

    #[instrument(level = "trace", skip(self), ret)]
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let mut iter = self.values.iter();
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
        self.visited_keys.lock().push(KeyVisit {
            visit_type: KeyVisitType::Next,
            visited_key: entry.as_ref().map(|(k, _)| *k),
        });
        Ok(entry)
    }
}

impl<T: Debug + Clone> HashedStorageCursor for MockHashedCursor<T> {
    #[instrument(level = "trace", skip(self), ret)]
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(self.values.is_empty())
    }
}
