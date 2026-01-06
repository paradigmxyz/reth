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
        Ok(DatabaseHashedAccountCursor(self.0.cursor_read::<tables::HashedAccounts>()?))
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
#[derive(Debug)]
pub struct DatabaseHashedAccountCursor<C>(C);

impl<C> DatabaseHashedAccountCursor<C> {
    /// Create new database hashed account cursor.
    pub const fn new(cursor: C) -> Self {
        Self(cursor)
    }
}

impl<C> HashedCursor for DatabaseHashedAccountCursor<C>
where
    C: DbCursorRO<tables::HashedAccounts>,
{
    type Value = Account;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.0.next()
    }

    fn reset(&mut self) {
        // Database cursors are stateless, no reset needed
    }
}

/// The structure wrapping a database cursor for hashed storage and
/// a target hashed address. Implements [`HashedCursor`] and [`HashedStorageCursor`]
/// for iterating over hashed storage.
///
/// This cursor implements locality optimization: when seeking forward from the current position,
/// it uses `next_dup_val` (O(1)) to walk forward instead of `seek_by_key_subkey` (O(log N)) when
/// the target key is close to the current position.
#[derive(Debug)]
pub struct DatabaseHashedStorageCursor<C> {
    /// Database hashed storage cursor.
    cursor: C,
    /// Target hashed address of the account that the storage belongs to.
    hashed_address: B256,
    /// The last key returned by the cursor, used for locality optimization.
    /// When seeking forward from this position, we can use `next_dup_val` instead of re-seeking.
    last_key: Option<B256>,
}

impl<C> DatabaseHashedStorageCursor<C> {
    /// Create new [`DatabaseHashedStorageCursor`].
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address, last_key: None }
    }
}

impl<C> HashedCursor for DatabaseHashedStorageCursor<C>
where
    C: DbCursorRO<tables::HashedStorages> + DbDupCursorRO<tables::HashedStorages>,
{
    type Value = U256;

    /// Seeks the given key in the hashed storage.
    ///
    /// This method implements a locality optimization: if the cursor is already positioned
    /// and the target key is >= the current position, we use `next_dup_val` to walk forward
    /// instead of performing an expensive `seek_by_key_subkey` operation.
    fn seek(&mut self, subkey: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Locality optimization: if cursor is positioned and target is ahead,
        // walk forward using next_dup_val instead of seeking
        if let Some(last) = self.last_key {
            if subkey > last {
                // Walk forward using next_dup_val until we find a key >= target
                while let Some(entry) = self.cursor.next_dup_val()? {
                    if entry.key >= subkey {
                        self.last_key = Some(entry.key);
                        return Ok(Some((entry.key, entry.value)));
                    }
                }
                // Exhausted the duplicates, no match found
                self.last_key = None;
                return Ok(None);
            } else if subkey == last &&
                let Some((_, entry)) = self.cursor.current()? &&
                entry.key == subkey
            {
                // Re-seeking the same key, return current position if still valid
                return Ok(Some((entry.key, entry.value)));
            }
        }

        // Fall back to seek_by_key_subkey for backward seeks or when cursor is not positioned
        let result =
            self.cursor.seek_by_key_subkey(self.hashed_address, subkey)?.map(|e| (e.key, e.value));
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let result = self.cursor.next_dup_val()?.map(|e| (e.key, e.value));
        self.last_key = result.as_ref().map(|(k, _)| *k);
        Ok(result)
    }

    fn reset(&mut self) {
        self.last_key = None;
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
        // Reset cursor position tracking when switching to a different storage
        self.last_key = None;
    }
}
