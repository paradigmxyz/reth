use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

/// Noop trie cursor factory.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    type AccountTrieCursor = NoopAccountTrieCursor;
    type StorageTrieCursor = NoopStorageTrieCursor;

    /// Generates a noop account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        Ok(NoopAccountTrieCursor::default())
    }

    /// Generates a noop storage trie cursor.
    fn storage_trie_cursor(
        &self,
        _hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
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
}
