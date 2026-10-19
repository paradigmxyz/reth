use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

/// Noop trie cursor factory.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    type AccountTrieCursor<'a>
        = NoopAccountTrieCursor
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = NoopStorageTrieCursor
    where
        Self: 'a;

    /// Generates a noop account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(NoopAccountTrieCursor::default())
    }

    /// Generates a noop storage trie cursor.
    fn storage_trie_cursor(
        &self,
        _hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(NoopStorageTrieCursor::default())
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopAccountTrieCursor;

impl TrieCursor for NoopAccountTrieCursor {
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn seek(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(None)
    }

    fn reset(&mut self) {
        // Noop
    }
}

/// Noop storage trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopStorageTrieCursor;

impl TrieCursor for NoopStorageTrieCursor {
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn seek(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(None)
    }

    fn reset(&mut self) {
        // Noop
    }
}

impl TrieStorageCursor for NoopStorageTrieCursor {
    fn set_hashed_address(&mut self, _hashed_address: B256) {
        // Noop
    }
}
