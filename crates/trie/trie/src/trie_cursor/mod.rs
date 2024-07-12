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

pub use self::{
    database_cursors::{DatabaseAccountTrieCursor, DatabaseStorageTrieCursor},
    in_memory::*,
    subnode::CursorSubNode,
};

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// The account trie cursor type.
    type AccountTrieCursor: TrieCursor;

    /// The storage trie cursor type.
    type StorageTrieCursor: TrieCursor;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Err>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, Self::Err>;
}

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursor: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(&mut self, key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<Nibbles>, Self::Err>;
}
