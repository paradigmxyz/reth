use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::{in_memory::*, subnode::CursorSubNode};

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// The account trie cursor type.
    type AccountTrieCursor: TrieCursor;
    /// The storage trie cursor type.
    type StorageTrieCursor: TrieCursor;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError>;
}

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursor: Send + Sync {
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
