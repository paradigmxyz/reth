use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO, ReusableCursor},
    tables::{AccountsHistory, PlainStorageState, StorageChangeSets, StoragesHistory},
    transaction::DbTx,
    DatabaseError,
};
use std::cell::Cell;

/// Container for reusable database cursors.
///
/// Holds optional cached cursors for frequently accessed tables. When a cursor is requested,
/// it returns a cached cursor if available, otherwise creates a new one. The cursor is
/// automatically returned to the cache when dropped via the `ReusableCursor` wrapper.
///
/// This reduces cursor allocation overhead for state providers that perform many sequential
/// database operations.
pub(crate) struct ReusableStateCursors<TX: DbTx> {
    storage_changesets: Cell<Option<TX::DupCursor<StorageChangeSets>>>,
    plain_storage_state: Cell<Option<TX::DupCursor<PlainStorageState>>>,
    accounts_history: Cell<Option<TX::Cursor<AccountsHistory>>>,
    storages_history: Cell<Option<TX::Cursor<StoragesHistory>>>,
}

impl<TX: DbTx> std::fmt::Debug for ReusableStateCursors<TX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReusableStateCursors").finish_non_exhaustive()
    }
}

impl<TX: DbTx> ReusableStateCursors<TX> {
    /// Creates a new `ReusableStateCursors` with empty cursor cells.
    pub(crate) const fn new() -> Self {
        Self {
            storage_changesets: Cell::new(None),
            plain_storage_state: Cell::new(None),
            accounts_history: Cell::new(None),
            storages_history: Cell::new(None),
        }
    }

    /// Gets a reusable cursor for the `StorageChangeSets` table.
    ///
    /// If a cursor is cached, it will be reused. Otherwise, a new cursor is created.
    pub(crate) fn storage_changesets<'tx, 'cell>(
        &'cell self,
        tx: &'tx TX,
    ) -> Result<
        ReusableCursor<'tx, 'cell, StorageChangeSets, TX::DupCursor<StorageChangeSets>>,
        DatabaseError,
    >
    where
        TX::DupCursor<StorageChangeSets>: DbDupCursorRO<StorageChangeSets>,
    {
        let cursor =
            self.storage_changesets.take().map(Ok).unwrap_or_else(|| tx.cursor_dup_read())?;
        Ok(ReusableCursor::new(cursor, &self.storage_changesets))
    }

    /// Gets a reusable cursor for the `PlainStorageState` table.
    ///
    /// If a cursor is cached, it will be reused. Otherwise, a new cursor is created.
    pub(crate) fn plain_storage_state<'tx, 'cell>(
        &'cell self,
        tx: &'tx TX,
    ) -> Result<
        ReusableCursor<'tx, 'cell, PlainStorageState, TX::DupCursor<PlainStorageState>>,
        DatabaseError,
    >
    where
        TX::DupCursor<PlainStorageState>: DbDupCursorRO<PlainStorageState>,
    {
        let cursor =
            self.plain_storage_state.take().map(Ok).unwrap_or_else(|| tx.cursor_dup_read())?;
        Ok(ReusableCursor::new(cursor, &self.plain_storage_state))
    }

    /// Gets a reusable cursor for the `AccountsHistory` table.
    ///
    /// If a cursor is cached, it will be reused. Otherwise, a new cursor is created.
    pub(crate) fn accounts_history<'tx, 'cell>(
        &'cell self,
        tx: &'tx TX,
    ) -> Result<
        ReusableCursor<'tx, 'cell, AccountsHistory, TX::Cursor<AccountsHistory>>,
        DatabaseError,
    >
    where
        TX::Cursor<AccountsHistory>: DbCursorRO<AccountsHistory>,
    {
        let cursor = self.accounts_history.take().map(Ok).unwrap_or_else(|| tx.cursor_read())?;
        Ok(ReusableCursor::new(cursor, &self.accounts_history))
    }

    /// Gets a reusable cursor for the `StoragesHistory` table.
    ///
    /// If a cursor is cached, it will be reused. Otherwise, a new cursor is created.
    pub(crate) fn storages_history<'tx, 'cell>(
        &'cell self,
        tx: &'tx TX,
    ) -> Result<
        ReusableCursor<'tx, 'cell, StoragesHistory, TX::Cursor<StoragesHistory>>,
        DatabaseError,
    >
    where
        TX::Cursor<StoragesHistory>: DbCursorRO<StoragesHistory>,
    {
        let cursor = self.storages_history.take().map(Ok).unwrap_or_else(|| tx.cursor_read())?;
        Ok(ReusableCursor::new(cursor, &self.storages_history))
    }
}

impl<TX: DbTx> Default for ReusableStateCursors<TX> {
    fn default() -> Self {
        Self::new()
    }
}
