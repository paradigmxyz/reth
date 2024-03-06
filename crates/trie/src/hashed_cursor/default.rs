use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_primitives::{Account, StorageEntry, B256};

impl<'a, TX: DbTx> HashedCursorFactory for &'a TX {
    type AccountCursor = <TX as DbTx>::Cursor<tables::HashedAccounts>;
    type StorageCursor = <TX as DbTx>::DupCursor<tables::HashedStorages>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.cursor_read::<tables::HashedAccounts>()
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        self.cursor_dup_read::<tables::HashedStorages>()
    }
}

impl<C> HashedAccountCursor for C
where
    C: DbCursorRO<tables::HashedAccounts>,
{
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Account)>, reth_db::DatabaseError> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Account)>, reth_db::DatabaseError> {
        self.next()
    }
}

impl<C> HashedStorageCursor for C
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
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
