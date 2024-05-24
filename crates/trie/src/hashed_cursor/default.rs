use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_primitives::{Account, StorageEntry, B256};

impl<'a, TX: DbTx> HashedCursorFactory for &'a TX {
    type AccountCursor = <TX as DbTx>::Cursor<tables::HashedAccounts>;
    type StorageCursor =
        DatabaseHashedStorageCursor<<TX as DbTx>::DupCursor<tables::HashedStorages>>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.cursor_read::<tables::HashedAccounts>()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.cursor_dup_read::<tables::HashedStorages>()?,
            hashed_address,
        ))
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

/// The structure wrapping a database cursor for hashed storage and
/// a target hashed address. Implements [HashedStorageCursor] for iterating
/// hashed state
#[derive(Debug)]
pub struct DatabaseHashedStorageCursor<C> {
    cursor: C,
    hashed_address: B256,
}

impl<C> DatabaseHashedStorageCursor<C> {
    /// Create new [DatabaseHashedStorageCursor].
    pub fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> HashedStorageCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    fn is_storage_empty(&mut self) -> Result<bool, reth_db::DatabaseError> {
        Ok(self.cursor.seek_exact(self.hashed_address)?.is_none())
    }

    fn seek(&mut self, subkey: B256) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.cursor.seek_by_key_subkey(self.hashed_address, subkey)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::DatabaseError> {
        self.cursor.next_dup_val()
    }
}
