use super::{TrieCursor, TrieCursorFactory};
use crate::updates::TrieKey;
use reth_db::DatabaseError;
use reth_primitives::trie::{BranchNodeCompact, StoredNibbles, StoredNibblesSubKey};

/// Noop trie cursor factory.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    /// Generates a Noop account trie cursor.
    fn account_trie_cursor(
        &self,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibbles> + '_>, DatabaseError> {
        Ok(Box::<NoopAccountTrieCursor>::default())
    }

    /// Generates a Noop storage trie cursor.
    fn storage_tries_cursor(
        &self,
        _hashed_address: reth_primitives::B256,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibblesSubKey> + '_>, DatabaseError> {
        Ok(Box::<NoopStorageTrieCursor>::default())
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopAccountTrieCursor;

impl TrieCursor for NoopAccountTrieCursor {
    type Key = StoredNibbles;

    /// Seeks within the account trie.
    fn seek(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    /// Seeks an exact match within the account trie.
    fn seek_exact(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    /// Retrieves the current cursor position within the account trie.
    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(None)
    }
}

/// Noop storage trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopStorageTrieCursor;

impl TrieCursor for NoopStorageTrieCursor {
    type Key = StoredNibblesSubKey;

    /// Seeks a key in storage tries.
    fn seek(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    /// Seeks an exact match in storage tries.
    fn seek_exact(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    /// Retrieves the current cursor position within storage tries.
    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(None)
    }
}
