use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::{DbTx, DbTxGAT},
};
use reth_primitives::{Account, StorageEntry, H256};

impl<'a, TX: DbTx<'a>> HashedCursorFactory<'a> for TX {
    type AccountCursor<'tx> = <TX as DbTxGAT<'tx>>::Cursor<tables::HashedAccount> where Self: 'tx;
    type StorageCursor<'tx> = <TX as DbTxGAT<'tx>>::DupCursor<tables::HashedStorage> where Self: 'tx;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, reth_db::Error> {
        self.cursor_read::<tables::HashedAccount>()
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor<'_>, reth_db::Error> {
        self.cursor_dup_read::<tables::HashedStorage>()
    }
}

impl<'tx, C> HashedAccountCursor for C
where
    C: DbCursorRO<'tx, tables::HashedAccount>,
{
    fn seek(&mut self, key: H256) -> Result<Option<(H256, Account)>, reth_db::Error> {
        self.seek(key)
    }

    fn next(&mut self) -> Result<Option<(H256, Account)>, reth_db::Error> {
        self.next()
    }
}

impl<'tx, C> HashedStorageCursor for C
where
    C: DbCursorRO<'tx, tables::HashedStorage> + DbDupCursorRO<'tx, tables::HashedStorage>,
{
    fn seek(&mut self, key: H256, subkey: H256) -> Result<Option<StorageEntry>, reth_db::Error> {
        self.seek_by_key_subkey(key, subkey)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::Error> {
        self.next_dup_val()
    }
}
