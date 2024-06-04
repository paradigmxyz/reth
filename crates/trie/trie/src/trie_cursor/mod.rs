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
