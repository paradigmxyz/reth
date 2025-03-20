use parking_lot::{Mutex, MutexGuard};
use std::{collections::BTreeMap, sync::Arc};

use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::{map::B256Map, B256};
use reth_storage_errors::db::DatabaseError;

/// Mock trie cursor factory.
#[derive(Clone, Default, Debug)]
pub struct MockTrieCursorFactory {
    account_trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
    storage_tries: B256Map<Arc<BTreeMap<Nibbles, BranchNodeCompact>>>,

    /// List of keys that the account trie cursor has visited.
    visited_account_keys: Arc<Mutex<Vec<Nibbles>>>,
    /// List of keys that the storage trie cursor has visited, per storage trie.
    visited_storage_keys: B256Map<Arc<Mutex<Vec<Nibbles>>>>,
}

impl MockTrieCursorFactory {
    /// Creates a new mock trie cursor factory.
    pub fn new(
        account_trie_nodes: BTreeMap<Nibbles, BranchNodeCompact>,
        storage_tries: B256Map<BTreeMap<Nibbles, BranchNodeCompact>>,
    ) -> Self {
        let visited_storage_keys = storage_tries.keys().map(|k| (*k, Default::default())).collect();
        Self {
            account_trie_nodes: Arc::new(account_trie_nodes),
            storage_tries: storage_tries.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
            visited_account_keys: Default::default(),
            visited_storage_keys,
        }
    }

    /// Returns a reference to the list of visited account keys.
    pub fn visited_account_keys(&self) -> MutexGuard<'_, Vec<Nibbles>> {
        self.visited_account_keys.lock()
    }

    /// Returns a reference to the list of visited storage keys for the given hashed address.
    pub fn visited_storage_keys(&self, hashed_address: B256) -> MutexGuard<'_, Vec<Nibbles>> {
        self.visited_storage_keys.get(&hashed_address).expect("storage trie should exist").lock()
    }
}

impl TrieCursorFactory for MockTrieCursorFactory {
    type AccountTrieCursor = MockTrieCursor;
    type StorageTrieCursor = MockTrieCursor;

    /// Generates a mock account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        Ok(MockTrieCursor::new(self.account_trie_nodes.clone(), self.visited_account_keys.clone()))
    }

    /// Generates a mock storage trie cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        Ok(MockTrieCursor::new(
            self.storage_tries
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

/// Mock trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct MockTrieCursor {
    /// The current key. If set, it is guaranteed to exist in `trie_nodes`.
    current_key: Option<Nibbles>,
    trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
    visited_keys: Arc<Mutex<Vec<Nibbles>>>,
}

impl MockTrieCursor {
    fn new(
        trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
        visited_keys: Arc<Mutex<Vec<Nibbles>>>,
    ) -> Self {
        Self { current_key: None, trie_nodes, visited_keys }
    }

    fn set_current_key(&mut self, key: &Nibbles) {
        self.current_key = Some(key.clone());
        self.visited_keys.lock().push(key.clone());
    }
}

impl TrieCursor for MockTrieCursor {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.trie_nodes.get(&key).cloned().map(|value| (key, value));
        if let Some((key, _)) = &entry {
            self.set_current_key(key);
        }
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        // Find the first key that has a prefix of the given key.
        let entry = self
            .trie_nodes
            .iter()
            .find_map(|(k, v)| k.starts_with(&key).then(|| (k.clone(), v.clone())));
        if let Some((key, _)) = &entry {
            self.set_current_key(key);
        }
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let mut iter = self.trie_nodes.iter();
        // Jump to the first key that has a prefix of the current key if it's set, or to the first
        // key otherwise.
        iter.find(|(k, _)| self.current_key.as_ref().is_none_or(|current| k.starts_with(current)))
            .expect("current key should exist in trie nodes");
        // Get the next key-value pair.
        let entry = iter
            .next()
            .or_else(|| self.trie_nodes.first_key_value())
            .map(|(k, v)| (k.clone(), v.clone()));
        if let Some((key, _)) = &entry {
            self.set_current_key(key);
        }
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.current_key.clone())
    }
}
