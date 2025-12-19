use reth_db_api::{
    cursor::{CursorGuard, DbCursorRO, DbDupCursorRO},
    tables::{AccountsHistory, PlainStorageState, StorageChangeSets, StoragesHistory},
    transaction::DbTx,
    DatabaseError,
};
use std::cell::RefCell;

/// Container for reusable database cursors.
///
/// Holds optional cached cursors for frequently accessed tables. When a cursor is requested,
/// it returns a cached cursor if available, otherwise creates a new one. The cursor is
/// automatically returned to the cache when dropped via the `CursorGuard` wrapper.
///
/// This reduces cursor allocation overhead for state providers that perform many sequential
/// database operations.
pub(crate) struct ReusableStateCursors<TX: DbTx> {
    storage_changesets: RefCell<Option<TX::DupCursor<StorageChangeSets>>>,
    plain_storage_state: RefCell<Option<TX::DupCursor<PlainStorageState>>>,
    accounts_history: RefCell<Option<TX::Cursor<AccountsHistory>>>,
    storages_history: RefCell<Option<TX::Cursor<StoragesHistory>>>,
}

impl<TX: DbTx> std::fmt::Debug for ReusableStateCursors<TX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReusableStateCursors").finish_non_exhaustive()
    }
}

impl<TX: DbTx> ReusableStateCursors<TX> {
    /// Creates a new `ReusableStateCursors` with empty cursor slots.
    pub(crate) const fn new() -> Self {
        Self {
            storage_changesets: RefCell::new(None),
            plain_storage_state: RefCell::new(None),
            accounts_history: RefCell::new(None),
            storages_history: RefCell::new(None),
        }
    }

    /// Gets a reusable cursor for the `StorageChangeSets` table.
    pub(crate) fn storage_changesets(
        &self,
        tx: &TX,
    ) -> Result<CursorGuard<'_, TX::DupCursor<StorageChangeSets>>, DatabaseError>
    where
        TX::DupCursor<StorageChangeSets>: DbDupCursorRO<StorageChangeSets>,
    {
        let cursor = self
            .storage_changesets
            .borrow_mut()
            .take()
            .map(Ok)
            .unwrap_or_else(|| tx.cursor_dup_read())?;
        Ok(CursorGuard::new(cursor, &self.storage_changesets))
    }

    /// Gets a reusable cursor for the `PlainStorageState` table.
    pub(crate) fn plain_storage_state(
        &self,
        tx: &TX,
    ) -> Result<CursorGuard<'_, TX::DupCursor<PlainStorageState>>, DatabaseError>
    where
        TX::DupCursor<PlainStorageState>: DbDupCursorRO<PlainStorageState>,
    {
        let cursor = self
            .plain_storage_state
            .borrow_mut()
            .take()
            .map(Ok)
            .unwrap_or_else(|| tx.cursor_dup_read())?;
        Ok(CursorGuard::new(cursor, &self.plain_storage_state))
    }

    /// Gets a reusable cursor for the `AccountsHistory` table.
    pub(crate) fn accounts_history(
        &self,
        tx: &TX,
    ) -> Result<CursorGuard<'_, TX::Cursor<AccountsHistory>>, DatabaseError>
    where
        TX::Cursor<AccountsHistory>: DbCursorRO<AccountsHistory>,
    {
        let cursor = self
            .accounts_history
            .borrow_mut()
            .take()
            .map(Ok)
            .unwrap_or_else(|| tx.cursor_read())?;
        Ok(CursorGuard::new(cursor, &self.accounts_history))
    }

    /// Gets a reusable cursor for the `StoragesHistory` table.
    pub(crate) fn storages_history(
        &self,
        tx: &TX,
    ) -> Result<CursorGuard<'_, TX::Cursor<StoragesHistory>>, DatabaseError>
    where
        TX::Cursor<StoragesHistory>: DbCursorRO<StoragesHistory>,
    {
        let cursor = self
            .storages_history
            .borrow_mut()
            .take()
            .map(Ok)
            .unwrap_or_else(|| tx.cursor_read())?;
        Ok(CursorGuard::new(cursor, &self.storages_history))
    }
}

impl<TX: DbTx> Default for ReusableStateCursors<TX> {
    fn default() -> Self {
        Self::new()
    }
}
