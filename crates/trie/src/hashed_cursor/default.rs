use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::{DbTx, DbTxGAT},
};
use reth_primitives::{Account, StorageEntry, B256};

impl<'a, 'tx, TX: DbTx<'tx>> HashedCursorFactory for &'a TX {
    type AccountCursor = <TX as DbTxGAT<'a>>::Cursor<tables::HashedAccount>;
    type StorageCursor = <TX as DbTxGAT<'a>>::DupCursor<tables::HashedStorage>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.cursor_read::<tables::HashedAccount>()
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        self.cursor_dup_read::<tables::HashedStorage>()
    }
}

impl<'tx, C> HashedAccountCursor for C
where
    C: DbCursorRO<'tx, tables::HashedAccount>,
{
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Account)>, reth_db::DatabaseError> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Account)>, reth_db::DatabaseError> {
        self.next()
    }
}

impl<'tx, C> HashedStorageCursor for C
where
    C: DbCursorRO<'tx, tables::HashedStorage> + DbDupCursorRO<'tx, tables::HashedStorage>,
{
    fn is_storage_empty(&mut self, key: B256) -> Result<bool, reth_db::DatabaseError> {
        Ok(self.seek_exact(key)?.is_none())
    }

    fn seek(
        &mut self,
        key: B256,
        subkey: B256,
    ) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.seek_by_key_subkey(key, subkey)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.next_dup_val()
    }
}
