use crate::{
    cursors::{
        DatabaseAccountChangeSetsCursor, DatabaseAccountTrieCursor,
        DatabaseStorageChangeSetsCursor, DatabaseStorageTrieCursor, DatabaseStoragesTrieCursor,
    },
    TxRefWrapper,
};
use reth_db::tables;
use reth_db_api::{
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_primitives::B256;
use reth_trie::trie_cursor::{TrieCursorFactory, TrieCursorRwFactory, TrieRangeWalkerFactory};

/// Implementation of the trie cursor factory for a database transaction.
impl<'a, TX: DbTx> TrieCursorFactory for TxRefWrapper<'a, TX> {
    type Err = DatabaseError;
    type AccountTrieCursor = DatabaseAccountTrieCursor<<TX as DbTx>::Cursor<tables::AccountsTrie>>;
    type StorageTrieCursor =
        DatabaseStorageTrieCursor<<TX as DbTx>::DupCursor<tables::StoragesTrie>>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Err> {
        Ok(DatabaseAccountTrieCursor::new(self.0.cursor_read::<tables::AccountsTrie>()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, Self::Err> {
        Ok(DatabaseStorageTrieCursor::new(
            self.0.cursor_dup_read::<tables::StoragesTrie>()?,
            hashed_address,
        ))
    }
}

/// Implementation of the mutable trie cursor factory for a database transaction.
impl<'a, TX: DbTx + DbTxMut> TrieCursorRwFactory for TxRefWrapper<'a, TX> {
    type Err = DatabaseError;
    type AccountTrieCursor =
        DatabaseAccountTrieCursor<<TX as DbTxMut>::CursorMut<tables::AccountsTrie>>;
    type StorageTrieCursor =
        DatabaseStoragesTrieCursor<<TX as DbTxMut>::DupCursorMut<tables::StoragesTrie>>;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, Self::Err> {
        Ok(DatabaseAccountTrieCursor::new(self.0.cursor_write::<tables::AccountsTrie>()?))
    }

    fn storage_trie_cursor(&self) -> Result<Self::StorageTrieCursor, Self::Err> {
        Ok(DatabaseStoragesTrieCursor::new(self.0.cursor_dup_write::<tables::StoragesTrie>()?))
    }
}

impl<'a, TX: DbTx> TrieRangeWalkerFactory for TxRefWrapper<'a, TX> {
    type Err = DatabaseError;
    type AccountCursor =
        DatabaseAccountChangeSetsCursor<<TX as DbTx>::Cursor<tables::AccountChangeSets>>;
    type StorageCursor =
        DatabaseStorageChangeSetsCursor<<TX as DbTx>::Cursor<tables::StorageChangeSets>>;

    fn account_change_sets(&self) -> Result<Self::AccountCursor, Self::Err> {
        Ok(DatabaseAccountChangeSetsCursor::new(self.0.cursor_read::<tables::AccountChangeSets>()?))
    }

    fn storage_change_sets(&self) -> Result<Self::StorageCursor, Self::Err> {
        Ok(DatabaseStorageChangeSetsCursor::new(self.0.cursor_read::<tables::StorageChangeSets>()?))
    }
}
