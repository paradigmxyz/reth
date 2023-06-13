use super::TrieCursor;
use crate::updates::TrieKey;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables, DatabaseError,
};
use reth_primitives::{
    trie::{BranchNodeCompact, StoredNibblesSubKey},
    H256,
};

/// A cursor over the storage trie.
pub struct StorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    hashed_address: H256,
}

impl<C> StorageTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub fn new(cursor: C, hashed_address: H256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<'a, C> TrieCursor<StoredNibblesSubKey> for StorageTrieCursor<C>
where
    C: DbDupCursorRO<'a, tables::StoragesTrie> + DbCursorRO<'a, tables::StoragesTrie>,
{
    fn seek_exact(
        &mut self,
        key: StoredNibblesSubKey,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key.clone())?
            .filter(|e| e.nibbles == key)
            .map(|value| (value.nibbles.inner.to_vec(), value.node)))
    }

    fn seek(
        &mut self,
        key: StoredNibblesSubKey,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key)?
            .map(|value| (value.nibbles.inner.to_vec(), value.node)))
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(k, v)| TrieKey::StorageNode(k, v.nibbles)))
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use reth_db::{
        cursor::DbCursorRW, mdbx::test_utils::create_test_rw_db, tables, transaction::DbTxMut,
    };
    use reth_primitives::{
        trie::{BranchNodeCompact, StorageTrieEntry},
        MAINNET,
    };
    use reth_provider::ShareableDatabase;

    // tests that upsert and seek match on the storagetrie cursor
    #[test]
    fn test_storage_cursor_abstraction() {
        let db = create_test_rw_db();
        let factory = ShareableDatabase::new(db.as_ref(), MAINNET.clone());
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = H256::random();
        let key = vec![0x2, 0x3];
        let value = BranchNodeCompact::new(1, 1, 1, vec![H256::random()], None);

        cursor
            .upsert(
                hashed_address,
                StorageTrieEntry { nibbles: key.clone().into(), node: value.clone() },
            )
            .unwrap();

        let mut cursor = StorageTrieCursor::new(cursor, hashed_address);
        assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
    }
}
