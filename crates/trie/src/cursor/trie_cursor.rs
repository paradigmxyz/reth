use crate::updates::TrieKey;
use reth_db::{table::Key, Error};
use reth_primitives::trie::BranchNodeCompact;

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
pub trait TrieCursor<K: Key> {
    /// Move the cursor to the key and return if it is an exact match.
    fn seek_exact(&mut self, key: K) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, Error>;

    /// Move the cursor to the key and return a value matching of greater than the key.
    fn seek(&mut self, key: K) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, Error>;

    /// Get the current entry.
    fn current(&mut self) -> Result<Option<TrieKey>, Error>;
}
