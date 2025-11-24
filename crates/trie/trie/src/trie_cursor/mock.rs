use parking_lot::{Mutex, MutexGuard};
use std::{collections::BTreeMap, sync::Arc};
use tracing::instrument;

use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use crate::{
    mock::{KeyVisit, KeyVisitType},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::{map::B256Map, B256};
use reth_storage_errors::db::DatabaseError;

/// Mock trie cursor factory.
#[derive(Clone, Default, Debug)]
pub struct MockTrieCursorFactory {
    account_trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
    storage_tries: Arc<B256Map<BTreeMap<Nibbles, BranchNodeCompact>>>,

    /// List of keys that the account trie cursor has visited.
    visited_account_keys: Arc<Mutex<Vec<KeyVisit<Nibbles>>>>,
    /// List of keys that the storage trie cursor has visited, per storage trie.
    visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<Nibbles>>>>>,
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
            storage_tries: Arc::new(storage_tries),
            visited_account_keys: Default::default(),
            visited_storage_keys: Arc::new(visited_storage_keys),
        }
    }

    /// Returns a reference to the list of visited account keys.
    pub fn visited_account_keys(&self) -> MutexGuard<'_, Vec<KeyVisit<Nibbles>>> {
        self.visited_account_keys.lock()
    }

    /// Returns a reference to the list of visited storage keys for the given hashed address.
    pub fn visited_storage_keys(
        &self,
        hashed_address: B256,
    ) -> MutexGuard<'_, Vec<KeyVisit<Nibbles>>> {
        self.visited_storage_keys.get(&hashed_address).expect("storage trie should exist").lock()
    }
}

impl TrieCursorFactory for MockTrieCursorFactory {
    type AccountTrieCursor<'a>
        = MockTrieCursor
    where
        Self: 'a;
    type StorageTrieCursor<'a>
        = MockTrieCursor
    where
        Self: 'a;

    /// Generates a mock account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(MockTrieCursor::new(self.account_trie_nodes.clone(), self.visited_account_keys.clone()))
    }

    /// Generates a mock storage trie cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        MockTrieCursor::new_storage(
            self.storage_tries.clone(),
            self.visited_storage_keys.clone(),
            hashed_address,
        )
    }
}

/// Mock trie cursor type - determines whether this is an account or storage cursor.
#[derive(Debug)]
enum MockTrieCursorType {
    Account {
        trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
        visited_keys: Arc<Mutex<Vec<KeyVisit<Nibbles>>>>,
    },
    Storage {
        all_storage_tries: Arc<B256Map<BTreeMap<Nibbles, BranchNodeCompact>>>,
        all_visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<Nibbles>>>>>,
        current_hashed_address: B256,
    },
}

/// Mock trie cursor.
#[derive(Debug)]
#[non_exhaustive]
pub struct MockTrieCursor {
    /// The current key. If set, it is guaranteed to exist in `trie_nodes`.
    current_key: Option<Nibbles>,
    cursor_type: MockTrieCursorType,
}

impl MockTrieCursor {
    /// Creates a new mock trie cursor for accounts with the given trie nodes and key tracking.
    pub fn new(
        trie_nodes: Arc<BTreeMap<Nibbles, BranchNodeCompact>>,
        visited_keys: Arc<Mutex<Vec<KeyVisit<Nibbles>>>>,
    ) -> Self {
        Self {
            current_key: None,
            cursor_type: MockTrieCursorType::Account { trie_nodes, visited_keys },
        }
    }

    /// Creates a new mock trie cursor for storage with access to all storage tries.
    pub fn new_storage(
        all_storage_tries: Arc<B256Map<BTreeMap<Nibbles, BranchNodeCompact>>>,
        all_visited_storage_keys: Arc<B256Map<Mutex<Vec<KeyVisit<Nibbles>>>>>,
        hashed_address: B256,
    ) -> Result<Self, DatabaseError> {
        if !all_storage_tries.contains_key(&hashed_address) {
            return Err(DatabaseError::Other(format!(
                "storage trie for {hashed_address:?} not found"
            )));
        }
        Ok(Self {
            current_key: None,
            cursor_type: MockTrieCursorType::Storage {
                all_storage_tries,
                all_visited_storage_keys,
                current_hashed_address: hashed_address,
            },
        })
    }

    /// Returns the trie nodes map for the current cursor type.
    fn trie_nodes(&self) -> &BTreeMap<Nibbles, BranchNodeCompact> {
        match &self.cursor_type {
            MockTrieCursorType::Account { trie_nodes, .. } => trie_nodes.as_ref(),
            MockTrieCursorType::Storage { all_storage_tries, current_hashed_address, .. } => {
                all_storage_tries
                    .get(current_hashed_address)
                    .expect("current_hashed_address should exist in all_storage_tries")
            }
        }
    }

    /// Returns the visited keys mutex for the current cursor type.
    fn visited_keys(&self) -> &Mutex<Vec<KeyVisit<Nibbles>>> {
        match &self.cursor_type {
            MockTrieCursorType::Account { visited_keys, .. } => visited_keys.as_ref(),
            MockTrieCursorType::Storage {
                all_visited_storage_keys,
                current_hashed_address,
                ..
            } => all_visited_storage_keys
                .get(current_hashed_address)
                .expect("current_hashed_address should exist in all_visited_storage_keys"),
        }
    }
}

impl TrieCursor for MockTrieCursor {
    #[instrument(skip(self), ret(level = "trace"))]
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.trie_nodes().get(&key).cloned().map(|value| (key, value));
        if let Some((key, _)) = &entry {
            self.current_key = Some(*key);
        }
        self.visited_keys().lock().push(KeyVisit {
            visit_type: KeyVisitType::SeekExact(key),
            visited_key: entry.as_ref().map(|(k, _)| *k),
        });
        Ok(entry)
    }

    #[instrument(skip(self), ret(level = "trace"))]
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        // Find the first key that is greater than or equal to the given key.
        let entry =
            self.trie_nodes().iter().find_map(|(k, v)| (k >= &key).then(|| (*k, v.clone())));
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
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let mut iter = self.trie_nodes().iter();
        // Jump to the first key that has a prefix of the current key if it's set, or to the first
        // key otherwise.
        iter.find(|(k, _)| self.current_key.as_ref().is_none_or(|current| k.starts_with(current)))
            .expect("current key should exist in trie nodes");
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

    #[instrument(skip(self), ret(level = "trace"))]
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.current_key)
    }

    fn reset(&mut self) {
        self.current_key = None;
    }
}

impl TrieStorageCursor for MockTrieCursor {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        match &mut self.cursor_type {
            MockTrieCursorType::Storage { current_hashed_address, .. } => {
                *current_hashed_address = hashed_address;
            }
            MockTrieCursorType::Account { .. } => {
                panic!("set_hashed_address called on account cursor")
            }
        }
    }
}
