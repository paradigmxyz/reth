/// Database implementations of trie cursors.
mod database_cursors;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::{database_cursors::*, in_memory::*, subnode::CursorSubNode};

use crate::{BranchNodeCompact, Nibbles};
use reth_db::DatabaseError;
use reth_primitives::B256;

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

/// Factory for creating mutable trie cursors.
pub trait TrieCursorRwFactory {
    /// The account trie cursor type.
    type AccountTrieCursor: TrieCursor + TrieCursorMut;
    /// The storage trie cursor type.
    type StorageTrieCursor: TrieDupCursor + TrieDupCursorMut;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(&self) -> Result<Self::StorageTrieCursor, DatabaseError>;
}

/// A cursor for navigating a trie that works with Tables.
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

/// A cursor for mutating a trie that works with Tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursorMut: Send + Sync {
    /// Delete entry at current cursor position.
    fn delete_current(&mut self) -> Result<(), DatabaseError>;

    /// Update existing entry or insert new one if it does not exist.
    fn upsert(&mut self, key: Nibbles, node: BranchNodeCompact) -> Result<(), DatabaseError>;
}

/// A cursor for navigating a trie that works with DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieDupCursor: Send + Sync {
    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: B256,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek_by_key_subkey(
        &mut self,
        key: B256,
        subkey: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError>;
}

/// A cursor for mutating a trie that works with DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieDupCursorMut: Send + Sync {
    /// Delete entry at current cursor position.
    fn delete_current(&mut self) -> Result<(), DatabaseError>;

    /// Delete entry at current cursor position and all its duplicates.
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError>;

    /// Update existing entry or insert new one if it does not exist.
    fn upsert(
        &mut self,
        key: B256,
        subkey: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError>;
}
