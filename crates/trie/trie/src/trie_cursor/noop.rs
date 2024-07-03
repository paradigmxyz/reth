use super::{TrieCursor, TrieCursorFactory};
use crate::{BranchNodeCompact, Nibbles};
use reth_primitives::B256;

/// Noop trie cursor factory.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    type Err = ();
    type AccountTrieCursor = NoopAccountTrieCursor;
    type StorageTrieCursor = NoopStorageTrieCursor;

    /// Generates a Noop account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, ()> {
        Ok(NoopAccountTrieCursor::default())
    }

    /// Generates a Noop storage trie cursor.
    fn storage_trie_cursor(&self, _hashed_address: B256) -> Result<Self::StorageTrieCursor, ()> {
        Ok(NoopStorageTrieCursor::default())
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopAccountTrieCursor;

impl TrieCursor for NoopAccountTrieCursor {
    type Err = ();

    /// Seeks an exact match within the account trie.
    fn seek_exact(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, ()> {
        Ok(None)
    }

    /// Seeks within the account trie.
    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, ()> {
        Ok(None)
    }

    /// Retrieves the current cursor position within the account trie.
    fn current(&mut self) -> Result<Option<Nibbles>, ()> {
        Ok(None)
    }
}

/// Noop storage trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopStorageTrieCursor;

impl TrieCursor for NoopStorageTrieCursor {
    type Err = ();

    /// Seeks an exact match in storage tries.
    fn seek_exact(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, ()> {
        Ok(None)
    }

    /// Seeks a key in storage tries.
    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, ()> {
        Ok(None)
    }

    /// Retrieves the current cursor position within storage tries.
    fn current(&mut self) -> Result<Option<Nibbles>, ()> {
        Ok(None)
    }
}
