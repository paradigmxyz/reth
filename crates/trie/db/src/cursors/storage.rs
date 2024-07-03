use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    DatabaseError,
};
use reth_primitives::B256;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieDupCursor, TrieDupCursorMut, TrieDupCursorRw},
    BranchNodeCompact, Nibbles, StoredNibblesSubKey,
};
use reth_trie_common::StorageTrieEntry;

/// A cursor over the storage tries stored in the database.
#[derive(Debug)]
pub struct DatabaseStorageTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
}

impl<C> DatabaseStorageTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> TrieCursor for DatabaseStorageTrieCursor<C>
where
    C: DbDupCursorRO<tables::StoragesTrie> + DbCursorRO<tables::StoragesTrie> + Send + Sync,
{
    type Err = DatabaseError;

    /// Seeks an exact match for the given key in the storage trie.
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key.clone()))?
            .filter(|e| e.nibbles == StoredNibblesSubKey(key))
            .map(|value| (value.nibbles.0, value.node)))
    }

    /// Seeks the given key in the storage trie.
    fn seek(&mut self, key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, StoredNibblesSubKey(key))?
            .map(|value| (value.nibbles.0, value.node)))
    }

    /// Retrieves the current value in the storage trie cursor.
    fn current(&mut self) -> Result<Option<Nibbles>, Self::Err> {
        Ok(self.cursor.current()?.map(|(_, v)| v.nibbles.0))
    }
}

/// A cursor over the storage tries stored in the database.
#[derive(Debug)]
pub struct DatabaseStoragesTrieCursor<C> {
    /// The underlying cursor.
    pub cursor: C,
}

impl<C> DatabaseStoragesTrieCursor<C> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self { cursor }
    }
}

impl<C> TrieDupCursor for DatabaseStoragesTrieCursor<C>
where
    C: DbDupCursorRO<tables::StoragesTrie> + DbCursorRO<tables::StoragesTrie> + Send + Sync,
{
    type Err = DatabaseError;

    fn seek_exact(&mut self, key: B256) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(self.cursor.seek_exact(key)?.map(|v| (v.1.nibbles.0, v.1.node)))
    }

    fn seek_by_key_subkey(
        &mut self,
        key: B256,
        subkey: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, Self::Err> {
        Ok(self
            .cursor
            .seek_by_key_subkey(key, StoredNibblesSubKey(subkey))?
            .map(|v| (v.nibbles.0, v.node)))
    }
}

impl<C> TrieDupCursorMut for DatabaseStoragesTrieCursor<C>
where
    C: DbDupCursorRW<tables::StoragesTrie> + DbCursorRW<tables::StoragesTrie> + Send + Sync,
{
    type Err = DatabaseError;

    fn delete_current(&mut self) -> Result<(), Self::Err> {
        self.cursor.delete_current()
    }

    fn delete_current_duplicates(&mut self) -> Result<(), Self::Err> {
        self.cursor.delete_current_duplicates()
    }

    fn upsert(
        &mut self,
        key: B256,
        subkey: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), Self::Err> {
        self.cursor.upsert(key, StorageTrieEntry { nibbles: StoredNibblesSubKey(subkey), node })
    }
}

impl<C> TrieDupCursorRw<<Self as TrieDupCursor>::Err, <Self as TrieDupCursorMut>::Err>
    for DatabaseStoragesTrieCursor<C>
where
    C: DbDupCursorRW<tables::StoragesTrie>
        + DbDupCursorRO<tables::StoragesTrie>
        + DbCursorRW<tables::StoragesTrie>
        + DbCursorRO<tables::StoragesTrie>
        + Send
        + Sync,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_trie::StorageTrieEntry;

    #[test]
    fn test_upsert_and_seek_match_on_storage_trie_cursor() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = B256::random();
        let key = StoredNibblesSubKey::from(vec![0x2, 0x3]);
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

        cursor
            .upsert(hashed_address, StorageTrieEntry { nibbles: key.clone(), node: value.clone() })
            .unwrap();

        let mut cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
        assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
    }
}
