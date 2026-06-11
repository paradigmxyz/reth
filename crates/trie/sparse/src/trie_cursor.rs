use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};

/// Factory for creating trie cursors.
#[auto_impl::auto_impl(&)]
pub trait TrieCursorFactory {
    /// The account trie cursor type.
    type AccountTrieCursor<'a>: TrieCursor
    where
        Self: 'a;

    /// The storage trie cursor type.
    type StorageTrieCursor<'a>: TrieStorageCursor
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
#[auto_impl::auto_impl(&mut, Box)]
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

    /// Reset the cursor to the beginning.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`TrieCursor::seek`] or
    /// [`TrieCursor::seek_exact`] call.
    fn reset(&mut self);
}

/// A cursor for traversing storage trie nodes.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieStorageCursor: TrieCursor {
    /// Set the hashed address for the storage trie cursor.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`TrieCursor::seek`] or
    /// [`TrieCursor::seek_exact`] call.
    fn set_hashed_address(&mut self, hashed_address: B256);
}
