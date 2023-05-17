use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::{DbTx, DbTxGAT},
};
use reth_primitives::{Account, StorageEntry, H256};

impl<'a, 'tx, TX: DbTx<'tx>> HashedCursorFactory<'a> for TX {
    type AccountCursor = <TX as DbTxGAT<'a>>::Cursor<tables::HashedAccount> where Self: 'a;
    type StorageCursor = <TX as DbTxGAT<'a>>::DupCursor<tables::HashedStorage> where Self: 'a;

    fn hashed_account_cursor(&'a self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.cursor_read::<tables::HashedAccount>()
    }

    fn hashed_storage_cursor(&'a self) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        self.cursor_dup_read::<tables::HashedStorage>()
    }
}

impl<'tx, C> HashedAccountCursor for C
where
    C: DbCursorRO<'tx, tables::HashedAccount>,
{
    fn seek(&mut self, key: H256) -> Result<Option<(H256, Account)>, reth_db::DatabaseError> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(H256, Account)>, reth_db::DatabaseError> {
        self.next()
    }
}

impl<'tx, C> HashedStorageCursor for C
where
    C: DbCursorRO<'tx, tables::HashedStorage> + DbDupCursorRO<'tx, tables::HashedStorage>,
{
    fn is_empty(&mut self, key: H256) -> Result<bool, reth_db::DatabaseError> {
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
