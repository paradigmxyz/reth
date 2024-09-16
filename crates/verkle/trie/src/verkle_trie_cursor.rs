use reth_storage_errors::db::DatabaseError;
use verkle_trie::{from_to_bytes::{FromBytes, ToBytes}, database::{memory_db::MemoryDb, meta::{BranchChild, BranchMeta, StemMeta}, Flush, ReadOnlyHigherDb, WriteOnlyHigherDb}};

/// Factory for creating verkle trie cursors.
pub trait VerkleTrieCursorFactory {
    /// The verkle trie cursor type.
    type VerkleTrieCursor: ReadOnlyHigherDb;

    /// Create a verkle trie cursor.
    fn verkle_trie_cursor(&self) -> Result<Self::VerkleTrieCursor, DatabaseError>;
}