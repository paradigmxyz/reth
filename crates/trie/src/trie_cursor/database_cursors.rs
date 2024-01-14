use super::{TrieCursor, TrieCursorFactory};
use crate::updates::TrieKey;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives::{
    trie::{BranchNodeCompact, StoredNibbles, StoredNibblesSubKey},
    B256,
};

/// Implementation of the trie cursor factory for a database transaction.
impl<'a, TX: DbTx> TrieCursorFactory for &'a TX {
    fn account_trie_cursor(
        &self,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibbles> + '_>, DatabaseError> {
        Ok(Box::new(DatabaseAccountTrieCursor::new(self.cursor_read::<tables::AccountsTrie>()?)))
    }

    fn storage_tries_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Box<dyn TrieCursor<Key = StoredNibblesSubKey> + '_>, DatabaseError> {
        Ok(Box::new(DatabaseStorageTrieCursor::new(
            self.cursor_dup_read::<tables::StoragesTrie>()?,
            hashed_address,
        )))
    }
}

/// A cursor over the account trie.
#[derive(Debug)]
pub struct DatabaseAccountTrieCursor<C>(C);

impl<C> DatabaseAccountTrieCursor<C> {
    /// Create a new account trie cursor.
    pub fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> TrieCursor for DatabaseAccountTrieCursor<C>
where
    C: DbCursorRO<tables::AccountsTrie>,
{
    /// The type of key used by this cursor.
    type Key = StoredNibbles;

    /// Seeks an exact match for the provided key in the account trie.
    fn seek_exact(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek_exact(key)?.map(|value| (value.0 .0.to_vec(), value.1 .0)))
    }

    /// Seeks a key in the account trie that matches or is greater than the provided key.
    fn seek(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self.0.seek(key)?.map(|value| (value.0 .0.to_vec(), value.1 .0)))
    }

    /// Retrieves the current key in the cursor.
    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(self.0.current()?.map(|(k, _)| TrieKey::AccountNode(k)))
    }
}

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
    pub fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> TrieCursor for DatabaseStorageTrieCursor<C>
where
    C: DbDupCursorRO<tables::StoragesTrie> + DbCursorRO<tables::StoragesTrie>,
{
    /// Defines the type for keys used in the storage trie cursor.
    type Key = StoredNibblesSubKey;

    /// Seeks an exact match for the given key in the storage trie.
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

    /// Seeks the given key in the storage trie.
    fn seek(
        &mut self,
        key: Self::Key,
    ) -> Result<Option<(Vec<u8>, BranchNodeCompact)>, DatabaseError> {
        Ok(self
            .cursor
            .seek_by_key_subkey(self.hashed_address, key)?
            .map(|value| (value.nibbles.to_vec(), value.node)))
    }

    /// Retrieves the current value in the storage trie cursor.
    fn current(&mut self) -> Result<Option<TrieKey>, DatabaseError> {
        Ok(self.cursor.current()?.map(|(k, v)| TrieKey::StorageNode(k, v.nibbles)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        cursor::{DbCursorRO, DbCursorRW},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::{
        hex_literal::hex,
        trie::{BranchNodeCompact, StorageTrieEntry, StoredBranchNode},
    };
    use reth_provider::test_utils::create_test_provider_factory;

    #[test]
    fn test_account_trie_order() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();

        let data = vec![
            hex!("0303040e").to_vec(),
            hex!("030305").to_vec(),
            hex!("03030500").to_vec(),
            hex!("0303050a").to_vec(),
        ];

        for key in data.clone() {
            cursor
                .upsert(
                    key.into(),
                    StoredBranchNode(BranchNodeCompact::new(
                        0b0000_0010_0000_0001,
                        0b0000_0010_0000_0001,
                        0,
                        Vec::default(),
                        None,
                    )),
                )
                .unwrap();
        }

        let db_data = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(db_data[0].0 .0.to_vec(), data[0]);
        assert_eq!(db_data[1].0 .0.to_vec(), data[1]);
        assert_eq!(db_data[2].0 .0.to_vec(), data[2]);
        assert_eq!(db_data[3].0 .0.to_vec(), data[3]);

        assert_eq!(
            cursor.seek(hex!("0303040f").to_vec().into()).unwrap().map(|(k, _)| k.0.to_vec()),
            Some(data[1].clone())
        );
    }

    // tests that upsert and seek match on the storage trie cursor
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

        let mut cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
        assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
    }
}
