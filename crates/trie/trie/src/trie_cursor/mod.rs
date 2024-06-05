use crate::updates::TrieKey;
use reth_primitives::{
    trie::{BranchNodeCompact, Nibbles},
    B256,
};

mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::subnode::CursorSubNode;

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// The associated error that can occur during the creation of the cursor.
    type Err;

    /// Create an account trie cursor.
    fn account_trie_cursor(&self) -> Result<Box<dyn TrieCursor<Err=Self::Err> + '_>, Self::Err>;

    /// Create a storage tries cursor.
    fn storage_tries_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Box<dyn TrieCursor<Err=Self::Err> + '_>, Self::Err>;
}

/// Factory for creating trie cursors.
pub trait TrieCursorRwFactory {
    /// The associated error that can occur during the creation of the cursor.
    type Err;
    type StorageKey;
    type StorageValue;
    type AccountsKey;
    type AccountsValue;

    /// Create an account trie cursor.
    fn account_trie_cursor_rw(&self) -> Result<Box<dyn TrieCursorRw<Self::AccountsKey, Self::AccountsValue, Err=Self::Err> + '_>, Self::Err>;

    /// Create a storage tries cursor.
    fn storage_tries_cursor_rw(
        &self,
    ) -> Result<Box<dyn TrieCursorRw<Self::StorageKey, Self::StorageValue, Err=Self::Err> + '_>, Self::Err>;
}

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursor: Send + Sync {
    /// The associated error that can occur on the cursor operations.
    type Err;

    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(&mut self, key: Nibbles)
        -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<TrieKey>, Self::Err>;
}

/// A cursor for writing into a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursorWrite<K, V>: Send + Sync {
    /// The associated error that can occur on the cursor operations.
    type Err;

    /// Deletes value at the current cursor position.
    fn delete_current(&mut self) -> Result<(), Self::Err>;

    /// Deletes all values associated with key at the current cursor position.
    fn delete_current_duplicates(&mut self) -> Result<(), Self::Err>;

    /// Updates `value` currently associated with `key` if it exists.
    /// Otherwise, inserts a new `key`-`value` pair.
    fn upsert(&mut self, key: K, value: V) -> Result<(), Self::Err>;
}

#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursorRw<K, V>: TrieCursor + TrieCursorWrite<K, V, Err=<Self as TrieCursor>::Err> + Send + Sync {
    /// The associated error that can occur on the cursor operations.
    type Err = <Self as TrieCursor>::Err;
}
