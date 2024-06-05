use super::{TrieCursor, TrieCursorFactory};
use crate::updates::TrieKey;
use reth_primitives::trie::{BranchNodeCompact, Nibbles};

/// Noop trie cursor factory.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopTrieCursorFactory;

impl TrieCursorFactory for NoopTrieCursorFactory {
    type Err = ();

    /// Generates a Noop account trie cursor.
    fn account_trie_cursor(&self) -> Result<Box<dyn TrieCursor<Err = Self::Err> + '_>, Self::Err> {
        Ok(Box::<NoopAccountTrieCursor>::default())
    }

    /// Generates a Noop storage trie cursor.
    fn storage_tries_cursor(
        &self,
        _hashed_address: reth_primitives::B256,
    ) -> Result<Box<dyn TrieCursor<Err = Self::Err> + '_>, Self::Err> {
        Ok(Box::<NoopStorageTrieCursor>::default())
    }
}

/// Noop account trie cursor.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct NoopAccountTrieCursor;

impl TrieCursor for NoopAccountTrieCursor {
    type Err = ();

    /// Seeks an exact match within the account trie.
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(None)
    }

    /// Seeks within the account trie.
    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(None)
    }

    /// Retrieves the current cursor position within the account trie.
    fn current(&mut self) -> Result<Option<TrieKey>, Self::Err> {
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
    fn seek_exact(
        &mut self,
        _key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(None)
    }

    /// Seeks a key in storage tries.
    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(None)
    }

    /// Retrieves the current cursor position within storage tries.
    fn current(&mut self) -> Result<Option<TrieKey>, Self::Err> {
        Ok(None)
    }
}
