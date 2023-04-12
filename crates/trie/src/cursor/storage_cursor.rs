use super::TrieCursor;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables, Error,
};
use reth_primitives::{
    trie::{BranchNodeCompact, StorageTrieEntry, StoredNibblesSubKey},
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
    C: DbDupCursorRO<'a, tables::StoragesTrie>
        + DbDupCursorRW<'a, tables::StoragesTrie>
        + DbCursorRO<'a, tables::StoragesTrie>
        + DbCursorRW<'a, tables::StoragesTrie>,
{
    fn seek_exact(
        &mut self,
        key: StoredNibblesSubKey,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, Error> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key.clone())?
            .filter(|e| e.nibbles == key)
            .map(|value| (value.nibbles.inner.to_vec(), value.node)))
    }

    fn seek(
        &mut self,
        key: StoredNibblesSubKey,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, Error> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key)?
            .map(|value| (value.nibbles.inner.to_vec(), value.node)))
    }

    fn upsert(&mut self, key: StoredNibblesSubKey, value: BranchNodeCompact) -> Result<(), Error> {
        if let Some(entry) = self.cursor.seek_by_key_subkey(self.hashed_address, key.clone())? {
            // "seek exact"
            if entry.nibbles == key {
                self.cursor.delete_current()?;
            }
        }

        self.cursor.upsert(self.hashed_address, StorageTrieEntry { nibbles: key, node: value })?;
        Ok(())
    }

    fn delete_current(&mut self) -> Result<(), Error> {
        self.cursor.delete_current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{mdbx::test_utils::create_test_rw_db, tables, transaction::DbTxMut};
    use reth_primitives::trie::BranchNodeCompact;
    use reth_provider::Transaction;

    // tests that upsert and seek match on the storagetrie cursor
    #[test]
    fn test_storage_cursor_abstraction() {
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();
        let cursor = tx.cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let mut cursor = StorageTrieCursor::new(cursor, H256::random());

        let key = vec![0x2, 0x3];
        let value = BranchNodeCompact::new(1, 1, 1, vec![H256::random()], None);

        cursor.upsert(key.clone().into(), value.clone()).unwrap();
        assert_eq!(cursor.seek(key.clone().into()).unwrap().unwrap().1, value);
    }
}
