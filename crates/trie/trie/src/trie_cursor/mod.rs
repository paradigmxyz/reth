use crate::{BranchNodeCompact, Nibbles};
use reth_primitives::{Account, Address, BlockNumber, B256, U256};
use reth_storage_errors::db::DatabaseError;
use std::ops::RangeBounds;

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

/// A trait for iterating change sets over a range of block numbers.
#[auto_impl::auto_impl(&mut, Box)]
pub trait ChangeSetWalker: Send + Sync {
    /// The associated change set being iterated over.
    type Value;

    /// Creates an iterator that walks over a range of block numbers.
    fn walk_range(
        &mut self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<impl Iterator<Item = Result<Self::Value, DatabaseError>>, DatabaseError>;
}

/// Creates readable change set iterators over a range of block numbers.
#[auto_impl::auto_impl(&mut, Box)]
pub trait ChangeSetWalkerFactory: Send + Sync {
    /// Cursor that iterates over a range of account change sets.
    type AccountCursor: ChangeSetWalker<Value = (Address, Option<Account>)>;

    /// Cursor that iterates over a range of storage change sets.
    type StorageCursor: ChangeSetWalker<Value = (Address, B256, U256)>;

    /// Creates account change sets cursor.
    fn account_change_sets(&self) -> Result<Self::AccountCursor, DatabaseError>;

    /// Creates storage change sets cursor.
    fn storage_change_sets(&self) -> Result<Self::StorageCursor, DatabaseError>;
}
