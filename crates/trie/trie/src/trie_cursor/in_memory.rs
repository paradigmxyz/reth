use super::{TrieCursor, TrieCursorFactory};
use crate::{
    forward_cursor::ForwardInMemoryCursor,
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
};
use reth_primitives::B256;
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
    type Err = CF::Err;
    type AccountTrieCursor = InMemoryAccountTrieCursor<'a, CF::AccountTrieCursor>;
    type StorageTrieCursor = InMemoryStorageTrieCursor<'a, CF::StorageTrieCursor>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Err> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(InMemoryAccountTrieCursor::new(cursor, self.trie_updates))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, Self::Err> {
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
#[allow(dead_code)]
pub struct InMemoryAccountTrieCursor<'a, C> {
    /// The database cursor.
    cursor: C,
    /// Forward-only in-memory cursor over storage trie nodes.
    in_memory_cursor: ForwardInMemoryCursor<'a, Nibbles, BranchNodeCompact>,
    /// Collection of removed trie nodes.
    removed_nodes: &'a HashSet<Nibbles>,
    /// Last key returned by the cursor.
    last_key: Option<Nibbles>,
}

impl<'a, C> InMemoryAccountTrieCursor<'a, C> {
    const fn new(cursor: C, trie_updates: &'a TrieUpdatesSorted) -> Self {
        let in_memory_cursor = ForwardInMemoryCursor::new(&trie_updates.account_nodes);
        Self {
            cursor,
            in_memory_cursor,
            removed_nodes: &trie_updates.removed_nodes,
            last_key: None,
        }
    }
}

impl<'a, C: TrieCursor> TrieCursor for InMemoryAccountTrieCursor<'a, C> {
    type Err = C::Err;

    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        unimplemented!()
    }

    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        unimplemented!()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, Self::Err> {
        unimplemented!()
    }
}

/// The cursor to iterate over storage trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InMemoryStorageTrieCursor<'a, C> {
    /// The hashed address of the account that trie belongs to.
    hashed_address: B256,
    /// The database cursor.
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
    fn new(hashed_address: B256, cursor: C, updates: Option<&'a StorageTrieUpdatesSorted>) -> Self {
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

impl<'a, C: TrieCursor> TrieCursor for InMemoryStorageTrieCursor<'a, C> {
    type Err = C::Err;

    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        unimplemented!()
    }

    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        unimplemented!()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, Self::Err> {
        unimplemented!()
    }
}
