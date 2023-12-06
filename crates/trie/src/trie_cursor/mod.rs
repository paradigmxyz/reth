use crate::updates::TrieKey;
use reth_db::DatabaseError;
use reth_primitives::trie::BranchNodeCompact;

mod account_cursor;
mod storage_cursor;
mod subnode;

pub use self::{
    account_cursor::AccountTrieCursor, storage_cursor::StorageTrieCursor, subnode::CursorSubNode,
};

/// A cursor for navigating a trie that works with both Tables and DupSort tables.
#[auto_impl::auto_impl(&mut)]
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
