use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_primitives::{Account, B256, U256};
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

/// New-type for a [`DbTx`] reference with [`HashedCursorFactory`] support.
pub struct DbTxRefWrapper<'a, TX: DbTx>(&'a TX);

/// New-type for a [`DbCursorRO`] reference with [`HashedCursor`] support.
pub struct DbCursorROWrapper<T: DbCursorRO<tables::HashedAccounts>>(T);

impl<'a, TX: DbTx> HashedCursorFactory for DbTxRefWrapper<'a, TX> {
    type AccountCursor = <TX as DbTx>::Cursor<tables::HashedAccounts>;
    type StorageCursor =
        DatabaseHashedStorageCursor<<TX as DbTx>::DupCursor<tables::HashedStorages>>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, reth_db::DatabaseError> {
        self.0.cursor_read::<tables::HashedAccounts>()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, reth_db::DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.0.cursor_dup_read::<tables::HashedStorages>()?,
            hashed_address,
        ))
    }
}

impl<C> HashedCursor for DbCursorROWrapper<C>
where
    C: DbCursorRO<tables::HashedAccounts>,
{
    type Err = reth_db::DatabaseError;
    type Value = Account;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, Self::Err> {
        self.0.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, Self::Err> {
        self.0.next()
    }
}

/// The structure wrapping a database cursor for hashed storage and
/// a target hashed address. Implements [`HashedCursor`] and [`HashedStorageCursor`]
/// for iterating over hashed storage.
#[derive(Debug)]
pub struct DatabaseHashedStorageCursor<C> {
    /// Database hashed storage cursor.
    cursor: C,
    /// Target hashed address of the account that the storage belongs to.
    hashed_address: B256,
}

impl<C> DatabaseHashedStorageCursor<C> {
    /// Create new [`DatabaseHashedStorageCursor`].
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address }
    }
}

impl<C> HashedCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    type Err = reth_db::DatabaseError;
    type Value = U256;

    fn seek(
        &mut self,
        subkey: B256,
    ) -> Result<Option<(B256, Self::Value)>, reth_db::DatabaseError> {
        Ok(self.cursor.seek_by_key_subkey(self.hashed_address, subkey)?.map(|e| (e.key, e.value)))
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, reth_db::DatabaseError> {
        Ok(self.cursor.next_dup_val()?.map(|e| (e.key, e.value)))
    }
}

impl<C> HashedStorageCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    fn is_storage_empty(&mut self) -> Result<bool, reth_db::DatabaseError> {
        Ok(self.cursor.seek_exact(self.hashed_address)?.is_none())
    }
}
