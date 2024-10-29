use super::{TrieCursor, TrieCursorFactory};
use crate::{
    forward_cursor::ForwardInMemoryCursor,
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};
use std::collections::HashSet;

/// The trie cursor factory for the trie updates.
#[derive(Debug, Clone)]
pub struct InMemoryTrieCursorFactory<'a, CF> {
    /// Underlying trie cursor factory.
    cursor_factory: CF,
    /// Reference to sorted trie updates.
    trie_updates: &'a TrieUpdatesSorted,
}

impl<'a, CF> InMemoryTrieCursorFactory<'a, CF> {
    /// Create a new trie cursor factory.
    pub const fn new(cursor_factory: CF, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor_factory, trie_updates }
    }
}

impl<'a, CF: TrieCursorFactory> TrieCursorFactory for InMemoryTrieCursorFactory<'a, CF> {
    type AccountTrieCursor = InMemoryAccountTrieCursor<'a, CF::AccountTrieCursor>;
    type StorageTrieCursor = InMemoryStorageTrieCursor<'a, CF::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(InMemoryAccountTrieCursor::new(cursor, self.trie_updates))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        let cursor = self.cursor_factory.storage_trie_cursor(hashed_address)?;
        Ok(InMemoryStorageTrieCursor::new(
            hashed_address,
            cursor,
            self.trie_updates.storage_tries.get(&hashed_address),
        ))
    }
}

/// The cursor to iterate over account trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
pub struct InMemoryAccountTrieCursor<'a, C> {
    /// The underlying cursor.
    cursor: C,
    /// Forward-only in-memory cursor over storage trie nodes.
    in_memory_cursor: ForwardInMemoryCursor<'a, Nibbles, BranchNodeCompact>,
    /// Collection of removed trie nodes.
    removed_nodes: &'a HashSet<Nibbles>,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
}

impl<'a, C: TrieCursor> InMemoryAccountTrieCursor<'a, C> {
    /// Create new account trie cursor from underlying cursor and reference to
    /// [`TrieUpdatesSorted`].
    pub const fn new(cursor: C, trie_updates: &'a TrieUpdatesSorted) -> Self {
        let in_memory_cursor = ForwardInMemoryCursor::new(&trie_updates.account_nodes);
        Self {
            cursor,
            in_memory_cursor,
            removed_nodes: &trie_updates.removed_nodes,
            last_key: None,
        }
    }

    fn seek_inner(
        &mut self,
        key: Nibbles,
        exact: bool,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let in_memory = self.in_memory_cursor.seek(&key);
        if exact && in_memory.as_ref().map_or(false, |entry| entry.0 == key) {
            return Ok(in_memory)
        }

        // Reposition the cursor to the first greater or equal node that wasn't removed.
        let mut db_entry = self.cursor.seek(key.clone())?;
        while db_entry.as_ref().map_or(false, |entry| self.removed_nodes.contains(&entry.0)) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        // If seek is exact, filter the entry for exact key match.
        Ok(compare_trie_node_entries(in_memory, db_entry)
            .filter(|(nibbles, _)| !exact || nibbles == &key))
    }

    fn next_inner(
        &mut self,
        last: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let in_memory = self.in_memory_cursor.first_after(&last);

        // Reposition the cursor to the first greater or equal node that wasn't removed.
        let mut db_entry = self.cursor.seek(last.clone())?;
        while db_entry
            .as_ref()
            .map_or(false, |entry| entry.0 < last || self.removed_nodes.contains(&entry.0))
        {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(compare_trie_node_entries(in_memory, db_entry))
    }
}

impl<C: TrieCursor> TrieCursor for InMemoryAccountTrieCursor<'_, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.seek_inner(key, true)?;
        self.last_key = entry.as_ref().map(|(nibbles, _)| nibbles.clone());
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.seek_inner(key, false)?;
        self.last_key = entry.as_ref().map(|(nibbles, _)| nibbles.clone());
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let next = match &self.last_key {
            Some(last) => {
                let entry = self.next_inner(last.clone())?;
                self.last_key = entry.as_ref().map(|entry| entry.0.clone());
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        match &self.last_key {
            Some(key) => Ok(Some(key.clone())),
            None => self.cursor.current(),
        }
    }
}

/// The cursor to iterate over storage trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InMemoryStorageTrieCursor<'a, C> {
    /// The hashed address of the account that trie belongs to.
    hashed_address: B256,
    /// The underlying cursor.
    cursor: C,
    /// Forward-only in-memory cursor over storage trie nodes.
    in_memory_cursor: Option<ForwardInMemoryCursor<'a, Nibbles, BranchNodeCompact>>,
    /// Reference to the set of removed storage node keys.
    removed_nodes: Option<&'a HashSet<Nibbles>>,
    /// The flag indicating whether the storage trie was cleared.
    storage_trie_cleared: bool,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
}

impl<'a, C> InMemoryStorageTrieCursor<'a, C> {
    /// Create new storage trie cursor from underlying cursor and reference to
    /// [`StorageTrieUpdatesSorted`].
    pub fn new(
        hashed_address: B256,
        cursor: C,
        updates: Option<&'a StorageTrieUpdatesSorted>,
    ) -> Self {
        let in_memory_cursor = updates.map(|u| ForwardInMemoryCursor::new(&u.storage_nodes));
        let removed_nodes = updates.map(|u| &u.removed_nodes);
        let storage_trie_cleared = updates.map_or(false, |u| u.is_deleted);
        Self {
            hashed_address,
            cursor,
            in_memory_cursor,
            removed_nodes,
            storage_trie_cleared,
            last_key: None,
        }
    }
}

impl<C: TrieCursor> InMemoryStorageTrieCursor<'_, C> {
    fn seek_inner(
        &mut self,
        key: Nibbles,
        exact: bool,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let in_memory = self.in_memory_cursor.as_mut().and_then(|c| c.seek(&key));
        if self.storage_trie_cleared ||
            (exact && in_memory.as_ref().map_or(false, |entry| entry.0 == key))
        {
            return Ok(in_memory.filter(|(nibbles, _)| !exact || nibbles == &key))
        }

        // Reposition the cursor to the first greater or equal node that wasn't removed.
        let mut db_entry = self.cursor.seek(key.clone())?;
        while db_entry.as_ref().map_or(false, |entry| {
            self.removed_nodes.as_ref().map_or(false, |r| r.contains(&entry.0))
        }) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        // If seek is exact, filter the entry for exact key match.
        Ok(compare_trie_node_entries(in_memory, db_entry)
            .filter(|(nibbles, _)| !exact || nibbles == &key))
    }

    fn next_inner(
        &mut self,
        last: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let in_memory = self.in_memory_cursor.as_mut().and_then(|c| c.first_after(&last));
        if self.storage_trie_cleared {
            return Ok(in_memory)
        }

        // Reposition the cursor to the first greater or equal node that wasn't removed.
        let mut db_entry = self.cursor.seek(last.clone())?;
        while db_entry.as_ref().map_or(false, |entry| {
            entry.0 < last || self.removed_nodes.as_ref().map_or(false, |r| r.contains(&entry.0))
        }) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(compare_trie_node_entries(in_memory, db_entry))
    }
}

impl<C: TrieCursor> TrieCursor for InMemoryStorageTrieCursor<'_, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.seek_inner(key, true)?;
        self.last_key = entry.as_ref().map(|(nibbles, _)| nibbles.clone());
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.seek_inner(key, false)?;
        self.last_key = entry.as_ref().map(|(nibbles, _)| nibbles.clone());
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let next = match &self.last_key {
            Some(last) => {
                let entry = self.next_inner(last.clone())?;
                self.last_key = entry.as_ref().map(|entry| entry.0.clone());
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        match &self.last_key {
            Some(key) => Ok(Some(key.clone())),
            None => self.cursor.current(),
        }
    }
}

/// Return the node with the lowest nibbles.
///
/// Given the next in-memory and database entries, return the smallest of the two.
/// If the node keys are the same, the in-memory entry is given precedence.
fn compare_trie_node_entries(
    mut in_memory_item: Option<(Nibbles, BranchNodeCompact)>,
    mut db_item: Option<(Nibbles, BranchNodeCompact)>,
) -> Option<(Nibbles, BranchNodeCompact)> {
    if let Some((in_memory_entry, db_entry)) = in_memory_item.as_ref().zip(db_item.as_ref()) {
        // If both are not empty, return the smallest of the two
        // In-memory is given precedence if keys are equal
        if in_memory_entry.0 <= db_entry.0 {
            in_memory_item.take()
        } else {
            db_item.take()
        }
    } else {
        // Return either non-empty entry
        db_item.or(in_memory_item)
    }
}
