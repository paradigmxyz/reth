use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use crate::database_provider::ConsistentDatabaseProvider;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives::{Account, StorageEntry, B256};

impl<DB: Database> HashedCursorFactory for ConsistentDatabaseProvider<DB> {
    type AccountCursor = <DB::TX as DbTx>::Cursor<tables::HashedAccount>;
    type StorageCursor = <DB::TX as DbTx>::DupCursor<tables::HashedStorage>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        self.tx()?.cursor_read::<tables::HashedAccount>()
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor, DatabaseError> {
        self.tx()?.cursor_dup_read::<tables::HashedStorage>()
    }
}

impl<'a, TX: DbTx> HashedCursorFactory for &'a TX {
    type AccountCursor = <TX as DbTx>::Cursor<tables::HashedAccount>;
    type StorageCursor = <TX as DbTx>::DupCursor<tables::HashedStorage>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        self.cursor_read::<tables::HashedAccount>()
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor, DatabaseError> {
        self.cursor_dup_read::<tables::HashedStorage>()
    }
}

impl<C> HashedAccountCursor for C
where
    C: DbCursorRO<tables::HashedAccount>,
{
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Account)>, DatabaseError> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Account)>, DatabaseError> {
        self.next()
    }
}

impl<C> HashedStorageCursor for C
where
    C: DbCursorRO<tables::HashedStorage> + DbDupCursorRO<tables::HashedStorage>,
{
    fn is_storage_empty(&mut self, key: B256) -> Result<bool, DatabaseError> {
        Ok(self.seek_exact(key)?.is_none())
    }

    fn seek(&mut self, key: B256, subkey: B256) -> Result<Option<StorageEntry>, DatabaseError> {
        self.seek_by_key_subkey(key, subkey)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, DatabaseError> {
        self.next_dup_val()
    }
}
