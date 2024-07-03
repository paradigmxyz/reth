use super::{TrieCursor, TrieCursorFactory};
use crate::updates::TrieUpdatesSorted;
use reth_db::DatabaseError;
use reth_primitives::B256;
use reth_trie_common::{BranchNodeCompact, Nibbles};

/// The trie cursor factory for the trie updates.
#[derive(Debug, Clone)]
pub struct InMemoryTrieCursorFactory<'a, CF> {
    cursor_factory: CF,
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
        Ok(InMemoryStorageTrieCursor::new(cursor, hashed_address, self.trie_updates))
    }
}

/// The cursor to iterate over account trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InMemoryAccountTrieCursor<'a, C> {
    cursor: C,
    trie_updates: &'a TrieUpdatesSorted,
    last_key: Option<Nibbles>,
}

impl<'a, C> InMemoryAccountTrieCursor<'a, C> {
    const fn new(cursor: C, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor, trie_updates, last_key: None }
    }
}

impl<'a, C: TrieCursor> TrieCursor for InMemoryAccountTrieCursor<'a, C> {
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        unimplemented!()
    }

    fn seek(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        unimplemented!()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        unimplemented!()
    }
}

/// The cursor to iterate over storage trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InMemoryStorageTrieCursor<'a, C> {
    cursor: C,
    trie_update_index: usize,
    trie_updates: &'a TrieUpdatesSorted,
    hashed_address: B256,
    last_key: Option<Nibbles>,
}

impl<'a, C> InMemoryStorageTrieCursor<'a, C> {
    const fn new(cursor: C, hashed_address: B256, trie_updates: &'a TrieUpdatesSorted) -> Self {
        Self { cursor, trie_updates, trie_update_index: 0, hashed_address, last_key: None }
    }
}

impl<'a, C: TrieCursor> TrieCursor for InMemoryStorageTrieCursor<'a, C> {
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        unimplemented!()
    }

    fn seek(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        unimplemented!()
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        unimplemented!()
    }
}
