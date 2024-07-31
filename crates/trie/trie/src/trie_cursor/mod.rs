use crate::{BranchNodeCompact, Nibbles};
use reth_primitives::B256;
use std::fmt::Debug;

/// Database implementations of trie cursors.
mod database_cursors;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::{database_cursors::*, in_memory::*, subnode::CursorSubNode};

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// Error returned by cursor types.
    type Error: Debug;
    /// The account trie cursor type.
    type AccountTrieCursor: TrieCursor<Error = Self::Error>;
    /// The storage trie cursor type.
    type StorageTrieCursor: TrieCursor<Error = Self::Error>;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Error>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, Self::Error>;
}

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursor: Send + Sync {
    /// Error returned by the cursor type.
    type Error: Debug;

    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Error>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(&mut self, key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Error>;

    /// Move the cursor to the next key.
    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Error>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<Nibbles>, Self::Error>;
}
