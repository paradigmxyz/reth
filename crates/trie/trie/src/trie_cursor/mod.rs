use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
pub mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

/// Depth-first trie iterator.
pub mod depth_first;

/// Mock trie cursor implementations.
#[cfg(test)]
pub mod mock;

pub use self::{depth_first::DepthFirstTrieIterator, in_memory::*, subnode::CursorSubNode};

/// Factory for creating trie cursors.
#[auto_impl::auto_impl(&)]
pub trait TrieCursorFactory {
    /// The account trie cursor type.
    type AccountTrieCursor<'a>: TrieCursor
    where
        Self: 'a;

    /// The storage trie cursor type.
    type StorageTrieCursor<'a>: TrieCursor
    where
        Self: 'a;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError>;
}

/// A cursor for traversing stored trie nodes. The cursor must iterate over keys in
/// lexicographical order.
#[auto_impl::auto_impl(&mut)]
pub trait TrieCursor {
    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(&mut self, key: Nibbles)
        -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError>;

    /// Move the cursor to the next key.
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError>;
}

/// Iterator wrapper for `TrieCursor` types
#[derive(Debug)]
pub struct TrieCursorIter<'a, C> {
    cursor: &'a mut C,
    /// The initial value from seek, if any
    initial: Option<Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
}

impl<'a, C> TrieCursorIter<'a, C> {
    /// Create a new iterator from a mutable reference to a cursor. The Iterator will start from the
    /// empty path.
    pub fn new(cursor: &'a mut C) -> Self
    where
        C: TrieCursor,
    {
        let initial = cursor.seek(Nibbles::default()).transpose();
        Self { cursor, initial }
    }
}

impl<'a, C> From<&'a mut C> for TrieCursorIter<'a, C>
where
    C: TrieCursor,
{
    fn from(cursor: &'a mut C) -> Self {
        Self::new(cursor)
    }
}

impl<'a, C> Iterator for TrieCursorIter<'a, C>
where
    C: TrieCursor,
{
    type Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // If we have an initial value from seek, return it first
        if let Some(initial) = self.initial.take() {
            return Some(initial);
        }

        self.cursor.next().transpose()
    }
}
