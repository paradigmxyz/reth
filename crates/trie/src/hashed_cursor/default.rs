use super::{HashedAccountsCursor, HashedCursorFactory, HashedStoragesCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::{DbTx, DbTxGAT},
};
use reth_primitives::{Account, StorageEntry, H256};

impl<'a, 'tx, TX: DbTx<'tx>> HashedCursorFactory<'a> for TX {
    type AccountCursor = <TX as DbTxGAT<'a>>::Cursor<tables::HashedAccounts> where Self: 'a;
    type StorageCursor = <TX as DbTxGAT<'a>>::DupCursor<tables::HashedStorages> where Self: 'a;

    fn hashed_account_cursor(&'a self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.cursor_read::<tables::HashedAccounts>()
    }

    fn hashed_storage_cursor(&'a self) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        self.cursor_dup_read::<tables::HashedStorages>()
    }
}

impl<'tx, C> HashedAccountsCursor for C
where
    C: DbCursorRO<'tx, tables::HashedAccounts>,
{
    fn seek(&mut self, key: H256) -> Result<Option<(H256, Account)>, reth_db::DatabaseError> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(H256, Account)>, reth_db::DatabaseError> {
        self.next()
    }
}

impl<'tx, C> HashedStoragesCursor for C
where
    C: DbCursorRO<'tx, tables::HashedStorages> + DbDupCursorRO<'tx, tables::HashedStorages>,
{
    fn is_storage_empty(&mut self, key: H256) -> Result<bool, reth_db::DatabaseError> {
        Ok(self.seek_exact(key)?.is_none())
    }

    fn seek(
        &mut self,
        key: H256,
        subkey: H256,
    ) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.seek_by_key_subkey(key, subkey)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.next_dup_val()
    }
}
