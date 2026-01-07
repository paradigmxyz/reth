use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseError,
};
use reth_primitives_traits::Account;
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

/// A struct wrapping database transaction that implements [`HashedCursorFactory`].
#[derive(Debug, Clone)]
pub struct DatabaseHashedCursorFactory<T>(T);

impl<T> DatabaseHashedCursorFactory<T> {
    /// Create new database hashed cursor factory.
    pub const fn new(tx: T) -> Self {
        Self(tx)
    }
}

impl<TX: DbTx> HashedCursorFactory for DatabaseHashedCursorFactory<&TX> {
    type AccountCursor<'a>
        = DatabaseHashedAccountCursor<<TX as DbTx>::Cursor<tables::HashedAccounts>>
    where
        Self: 'a;
    type StorageCursor<'a>
        = DatabaseHashedStorageCursor<<TX as DbTx>::DupCursor<tables::HashedStorages>>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedAccountCursor::new(self.0.cursor_read::<tables::HashedAccounts>()?))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.0.cursor_dup_read::<tables::HashedStorages>()?,
            hashed_address,
        ))
    }
}

/// A struct wrapping database cursor over hashed accounts implementing [`HashedCursor`] for
/// iterating over accounts.
///
/// This cursor implements a locality optimization: when seeking forward, it first tries
/// `next()` (O(1)) before falling back to `seek()` (O(log N)). This is effective because
/// trie traversal often accesses accounts in lexicographic order of their hashed addresses.
#[derive(Debug)]
pub struct DatabaseHashedAccountCursor<C> {
    /// The underlying database cursor.
    cursor: C,
    /// The last key returned by this cursor, used to detect forward seeks.
    last_key: Option<B256>,
}

impl<C> DatabaseHashedAccountCursor<C> {
    /// Create new database hashed account cursor.
    pub const fn new(cursor: C) -> Self {
        Self { cursor, last_key: None }
    }
}

impl<C> HashedCursor for DatabaseHashedAccountCursor<C>
where
    C: DbCursorRO<tables::HashedAccounts>,
{
    type Value = Account;

    /// Seeks a key in the hashed accounts table that matches or is greater than the provided key.
    ///
    /// Uses locality optimization: when seeking forward from the last position, tries `next()`
    /// first. If `next()` returns a key >= target, we avoid an O(log N) seek.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Locality optimization: if we're seeking forward, try next() first
        if let Some(last) = self.last_key
            && key > last
                && let Some((found_key, value)) = self.cursor.next()?
                    && found_key >= key {
                        // next() gave us a key >= target, we're done
                        self.last_key = Some(found_key);
                        return Ok(Some((found_key, value)));
                    }
                    // next() returned a key < target, need to seek

        let result = self.cursor.seek(key)?;
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let result = self.cursor.next()?;
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn reset(&mut self) {
        self.last_key = None;
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
    type Value = U256;

    fn seek(&mut self, subkey: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(self.cursor.seek_by_key_subkey(self.hashed_address, subkey)?.map(|e| (e.key, e.value)))
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(self.cursor.next_dup_val()?.map(|e| (e.key, e.value)))
    }

    fn reset(&mut self) {
        // Database cursors are stateless, no reset needed
    }
}

impl<C> HashedStorageCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(self.cursor.seek_exact(self.hashed_address)?.is_none())
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
    }
}
