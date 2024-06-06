use crate::updates::TrieKey;
use reth_primitives::{
    trie::{BranchNodeCompact, Nibbles, StorageTrieEntry, StoredBranchNode, StoredNibbles},
    B256,
};
use std::fmt::Debug;

mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::subnode::CursorSubNode;

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// The associated error that can occur during the creation of the cursor.
    type Err: Debug;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Box<dyn TrieCursor<Err = Self::Err> + '_>, Self::Err>;

    /// Create a storage tries cursor.
    fn storage_tries_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Box<dyn TrieCursor<Err = Self::Err> + '_>, Self::Err>;
}

/// Factory for creating trie cursors.
pub trait TrieCursorRwFactory {
    /// The associated error that can occur during the creation of the cursor.
    type Err: Debug;

    /// Create an account trie cursor.
    fn account_trie_cursor_rw(
        &self,
    ) -> Result<
        Box<dyn TrieCursorRw<StoredNibbles, StoredBranchNode, Err = Self::Err> + '_>,
        Self::Err,
    >;

    /// Create a storage tries cursor.
    fn storage_tries_cursor_rw(
        &self,
    ) -> Result<Box<dyn TrieCursorRw<B256, StorageTrieEntry, Err = Self::Err> + '_>, Self::Err>;
}

#[auto_impl::auto_impl(Box)]
pub trait TrieCursorErr: Send + Sync {
    type Err: Debug;
}

#[auto_impl::auto_impl(Box)]
/// A cursor for navigating a trie that works with both Tables and DupSort tables.
pub trait TrieCursor: Send + Sync + TrieCursorErr {
    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, <Box<Self> as TrieCursorErr>::Err>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, <Box<Self> as TrieCursorErr>::Err>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<TrieKey>, <Box<Self> as TrieCursorErr>::Err>;
}

/// A cursor for writing into a trie that works with both Tables and DupSort tables.
pub trait TrieCursorWrite<K, V>: Send + Sync + TrieCursorErr {
    /// Deletes value at the current cursor position.
    fn delete_current(&mut self) -> Result<(), Self::Err>;

    /// Deletes all values associated with key at the current cursor position.
    fn delete_current_duplicates(&mut self) -> Result<(), Self::Err>;

    /// Updates `value` currently associated with `key` if it exists.
    /// Otherwise, inserts a new `key`-`value` pair.
    fn upsert(&mut self, key: K, value: V) -> Result<(), Self::Err>;
}

#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursorRw<K, V>: TrieCursor + TrieCursorWrite<K, V> + Send + Sync {}
