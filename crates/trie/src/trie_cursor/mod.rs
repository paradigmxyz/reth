use crate::updates::TrieKey;
use reth_db::DatabaseError;
use reth_primitives::{
    trie::{BranchNodeCompact, StoredNibbles, StoredNibblesSubKey},
    B256,
};

mod database_cursors;
mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

pub use self::{
    database_cursors::{DatabaseAccountTrieCursor, DatabaseStorageTrieCursor},
    subnode::CursorSubNode,
};

/// Factory for creating trie cursors.
pub trait TrieCursorFactory {
    /// Create an account trie cursor.
    fn account_trie_cursor(
        &self,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibbles> + '_>, DatabaseError>;

    /// Create a storage tries cursor.
    fn storage_tries_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibblesSubKey> + '_>, DatabaseError>;
}

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut, Box)]
pub trait TrieCursor {
    /// The key type of the cursor.
    type Key: From<Vec<u8>>;

    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError>;
}
