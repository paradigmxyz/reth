use reth_db::DatabaseError;
use reth_primitives::trie::{BranchNodeCompact, StoredNibbles, StoredNibblesSubKey};

use crate::updates::TrieKey;

use super::{TrieCursor, TrieCursorFactory};

/// Noop trie cursor factory.
#[derive(Default, Debug)]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    fn account_trie_cursor(
        &self,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibbles> + '_>, DatabaseError> {
        Ok(Box::new(NoopAccountTrieCursor::default()))
    }

    fn storage_tries_cursor(
        &self,
        _hashed_address: reth_primitives::B256,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibblesSubKey> + '_>, DatabaseError> {
        Ok(Box::new(NoopStorageTrieCursor::default()))
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
pub struct NoopAccountTrieCursor;

impl TrieCursor for NoopAccountTrieCursor {
    type Key = StoredNibbles;

    fn seek(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn seek_exact(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(None)
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
pub struct NoopStorageTrieCursor;

impl TrieCursor for NoopStorageTrieCursor {
    type Key = StoredNibblesSubKey;

    fn seek(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn seek_exact(
        &mut self,
        _key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(None)
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(None)
    }
}
