use crate::{BranchNodeCompact, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

/// In-memory implementations of trie cursors.
mod in_memory;

/// Cursor for iterating over a subtrie.
pub mod subnode;

/// Noop trie cursor implementations.
pub mod noop;

/// Mock trie cursor implementations.
#[cfg(test)]
pub mod mock;

pub use self::{in_memory::*, subnode::CursorSubNode};

/// Factory for creating trie cursors.
#[auto_impl::auto_impl(&)]
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

/// A cursor for navigating a trie that works with both Tables and `DupSort` tables.
///
/// # Ordering assumptions
///
/// MDBX stores keys in **lexicographic byte order**. The trie uses this property for
/// traversal and proof construction:
///
/// - [`TrieWalker`] depends on lexicographic ordering to walk the trie in key order,
///   which is required for proof fetching.
/// - [`TrieNodeIter`] also relies on lexicographic ordering when iterating over nodes,
///   but may additionally use hashed lookups for storage tries (see its own docs).
///
/// These behaviors are surfaced through the [`TrieCursor`] API. Implementations must
/// preserve the underlying MDBX ordering guarantees when seeking, advancing, or reading
/// entries.
///
/// Consumers of this trait (e.g. [`TrieWalker`], [`TrieNodeIter`]) can rely on this
/// documented behavior instead of duplicating assumptions.

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
