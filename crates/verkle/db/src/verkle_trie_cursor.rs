use reth_db::{
    cursor::{DbCursorRW, DbDupCursorRW},
    tables, transaction::DbTxMut
};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    updates::StorageTrieUpdates,
    BranchNodeCompact, Nibbles, StoredNibbles, StoredNibblesSubKey,
};
use reth_trie_common::StorageTrieEntry;
use reth_verkle_trie::verkle_trie_cursor::VerkleTrieCursorFactory;
use verkle_trie::{from_to_bytes::{FromBytes, ToBytes}, database::{memory_db::MemoryDb, meta::{BranchChild, BranchMeta, StemMeta}, Flush, ReadOnlyHigherDb, WriteOnlyHigherDb}};

const CACHE_DEPTH: u8 = 4;
pub(crate) const LEAF_TABLE_MARKER: u8 = 0;
pub(crate) const STEM_TABLE_MARKER: u8 = 1;
pub(crate) const BRANCH_TABLE_MARKER: u8 = 2;

/// Wrapper struct for database transaction for verkle trie cursor.
#[derive(Debug)]
pub struct DatabaseVerkleTrie<'a, TX>(pub &'a TX);

impl<'a, TX>Clone for DatabaseVerkleTrie<'a, TX> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<'a, TX> DatabaseVerkleTrie<'a, TX> {

    /// Create new [`DatabaseVerkleTrie`].
    pub const fn new(tx: &'a TX) -> Self {
        Self(tx)
    }
}

// impl<'a, TX: DbTx> VerkleTrieCursorFactory for DatabaseVerkleTrie<'a, TX>
// where
//     <TX as DbTx>::Cursor<tables::VerkleTrie>: Clone,
// {
//     type VerkleTrieCursor = DatabaseVerkleTrieCursor<<TX as DbTx>::Cursor<tables::VerkleTrie>>;

//     fn verkle_trie_cursor(&self) -> Result<Self::VerkleTrieCursor, DatabaseError> {
//         Ok(DatabaseVerkleTrieCursor::new(self.0.cursor_read::<tables::VerkleTrie>()?))
//     }
// }
/// A cursor over the verkle trie.
#[derive(Debug)]
pub struct DatabaseVerkleTrieCursor<C>(pub(crate) C);

// impl<C> DatabaseVerkleTrieCursor<C> {

// }

impl<C> DatabaseVerkleTrieCursor<C>
where
    C: DbCursorRO<tables::VerkleTrie> + Send + Sync + Clone,
{
        /// Create new [`DatabaseVerkleTrieCursor`].
        pub const fn new(cursor: C) -> Self {
            Self(cursor)
        }
        
    pub fn get_leaf(&self, key: [u8; 32]) -> Option<[u8; 32]> {
        let mut cursor = self.0.clone();
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(LEAF_TABLE_MARKER);
        labelled_key.extend_from_slice(&key);

        cursor.seek_exact(labelled_key)
            .ok()
            .and_then(|result| result)
            .and_then(|(_, value)| value.try_into().ok())
    }

    fn get_stem_meta(&self, stem_key: [u8; 31]) -> Option<StemMeta> {
        let mut cursor = self.0.clone();
        let mut labelled_key = Vec::with_capacity(stem_key.len() + 1);
        labelled_key.push(STEM_TABLE_MARKER);
        labelled_key.extend_from_slice(&stem_key);

        cursor.seek_exact(labelled_key)
            .ok()
            .and_then(|result| result)
            .map(|(_, value)| StemMeta::from_bytes(value).unwrap())
    }

    fn get_branch_children(&self, branch_id: &[u8]) -> Vec<(u8, BranchChild)> {
        let mut cursor = self.0.clone();
        let mut children = Vec::with_capacity(256);

        let mut labelled_key = Vec::with_capacity(branch_id.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(branch_id);

        if let Ok(Some(_)) = cursor.seek(labelled_key.clone()) {
            while let Ok(Some((key, value))) = cursor.next() {
                if key.len() == labelled_key.len() + 1 && key.starts_with(&labelled_key) {
                    let index = key[key.len() - 1];
                    children.push((index, BranchChild::from_bytes(value).unwrap()));
                } else {
                    break;
                }
            }
        }

        children
    }

    fn get_branch_meta(&self, key: &[u8]) -> Option<BranchMeta> {
        let mut cursor = self.0.clone();
        let mut labelled_key = Vec::with_capacity(key.len() + 1);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(key);

        cursor.seek_exact(labelled_key)
            .ok()
            .and_then(|result| result)
            .map(|(_, value)| BranchMeta::from_bytes(value).unwrap())
    }

    fn get_branch_child(&self, branch_id: &[u8], index: u8) -> Option<BranchChild> {
        let mut cursor = self.0.clone();
        let mut labelled_key = Vec::with_capacity(branch_id.len() + 2);
        labelled_key.push(BRANCH_TABLE_MARKER);
        labelled_key.extend_from_slice(branch_id);
        labelled_key.push(index);

        cursor.seek_exact(labelled_key)
            .ok()
            .and_then(|result| result)
            .map(|(_, value)| BranchChild::from_bytes(value).unwrap())
    }

    fn get_stem_children(&self, stem_key: [u8; 31]) -> Vec<(u8, [u8; 32])> {
        let mut cursor = self.0.clone();
        let mut children = Vec::with_capacity(256);

        let mut labelled_key = Vec::with_capacity(stem_key.len() + 1);
        labelled_key.push(LEAF_TABLE_MARKER);
        labelled_key.extend_from_slice(&stem_key);

        if let Ok(Some(_)) = cursor.seek(labelled_key.clone()) {
            while let Ok(Some((key, value))) = cursor.next() {
                if key.len() == labelled_key.len() + 1 && key.starts_with(&labelled_key) {
                    let index = key[key.len() - 1];
                    children.push((index, value.try_into().unwrap()));
                } else {
                    break;
                }
            }
        }

        children
    }
}