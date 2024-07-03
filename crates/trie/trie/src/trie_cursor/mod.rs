use crate::{BranchNodeCompact, Nibbles};
use reth_primitives::{Account, Address, BlockNumber, B256, U256};
use std::{fmt::Debug, ops::RangeInclusive};

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::{in_memory::*, subnode::CursorSubNode};

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

/// Factory for creating mutable trie cursors.
pub trait TrieCursorRwFactory {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;
    /// The account trie cursor type.
    type AccountTrieCursor: TrieCursorRw<Self::Err, Self::Err>;
    /// The storage trie cursor type.
    type StorageTrieCursor: TrieDupCursorRw<Self::Err, Self::Err>;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Err>;

    /// Create a storage tries cursor.
    fn storage_trie_cursor(&self) -> Result<Self::StorageTrieCursor, Self::Err>;
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

/// A cursor for mutating a trie that works with Tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursorMut: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Delete entry at current cursor position.
    fn delete_current(&mut self) -> Result<(), Self::Err>;

    /// Update existing entry or insert new one if it does not exist.
    fn upsert(&mut self, key: Nibbles, node: BranchNodeCompact) -> Result<(), Self::Err>;
}

/// A readable and mutable cursor for Tables.
#[auto_impl::auto_impl(&mut, Box, TrieCursor + TrieCursorMut)]
pub trait TrieCursorRw<Err1, Err2>: TrieCursor<Err = Err1> + TrieCursorMut<Err = Err2> {}

/// A readable and mutable cursor for DubSort tables.
#[auto_impl::auto_impl(&mut, Box, TrieDupCursor + TrieDupCursorMut)]
pub trait TrieDupCursorRw<Err1, Err2>:
    TrieDupCursor<Err = Err1> + TrieDupCursorMut<Err = Err2>
{
}

/// A cursor for navigating a trie that works with DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieDupCursor: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(&mut self, key: B256) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek_by_key_subkey(
        &mut self,
        key: B256,
        subkey: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;
}

/// A cursor for mutating a trie that works with DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieDupCursorMut: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Delete entry at current cursor position.
    fn delete_current(&mut self) -> Result<(), Self::Err>;

    /// Delete entry at current cursor position and all its duplicates.
    fn delete_current_duplicates(&mut self) -> Result<(), Self::Err>;

    /// Update existing entry or insert new one if it does not exist.
    fn upsert(
        &mut self,
        key: B256,
        subkey: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), Self::Err>;
}

/// A trait for creating iterators walking over a range of block numbers.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieRangeWalker<V>: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Creates an iterator that walks over a range of block numbers.
    fn walk_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<impl Iterator<Item = Result<V, Self::Err>>, Self::Err>;
}

/// Walker factory
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieRangeWalkerFactory: Send + Sync {
    /// The associated error which can be returned from operations on the cursor.
    type Err: Debug;

    /// Cursor that walks over account change sets.
    type AccountCursor: TrieRangeWalker<(Address, Option<Account>), Err = Self::Err>;

    /// Cursor that walks over storage change sets.
    type StorageCursor: TrieRangeWalker<(Address, B256, U256), Err = Self::Err>;

    /// Creates account change sets cursor.
    fn account_change_sets(&self) -> Result<Self::AccountCursor, Self::Err>;

    /// Creates storage change sets cursor.
    fn storage_change_sets(&self) -> Result<Self::StorageCursor, Self::Err>;
}
