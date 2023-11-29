use super::TrieCursor;
use crate::updates::TrieKey;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables, DatabaseError,
};
use reth_primitives::{
    trie::{BranchNodeCompact, StoredNibblesSubKey},
    B256,
};

/// A cursor over the storage trie.
#[derive(Debug)]
pub struct StorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    hashed_address: B256,
}

impl<C> StorageTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> TrieCursor for StorageTrieCursor<C>
where
    C: DbDupCursorRO<tables::StoragesTrie> + DbCursorRO<tables::StoragesTrie>,
{
    type Key = StoredNibblesSubKey;

    fn seek_exact(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key.clone())?
            .filter(|e| e.nibbles == key)
            .map(|value| (value.nibbles.to_vec(), value.node)))
    }

    fn seek(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key)?
            .map(|value| (value.nibbles.to_vec(), value.node)))
    }

    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(k, v)| TrieKey::StorageNode(k, v.nibbles)))
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use reth_db::{cursor::DbCursorRW, tables, transaction::DbTxMut};
    use reth_primitives::trie::{BranchNodeCompact, StorageTrieEntry};
    use reth_provider::test_utils::create_test_provider_factory;

    // tests that upsert and seek match on the storagetrie cursor
    #[test]
    fn test_storage_cursor_abstraction() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = B256::random();
        let key = vec![0x2, 0x3];
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

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
