use reth_db_api::{
    cursor::{DbDupCursorRO, ReusableCursor},
    tables::PlainStorageState,
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
    plain_storage_state: Cell<Option<TX::DupCursor<PlainStorageState>>>,
}

impl<TX: DbTx> std::fmt::Debug for ReusableStateCursors<TX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReusableStateCursors").finish_non_exhaustive()
    }
}

impl<TX: DbTx> ReusableStateCursors<TX> {
    /// Creates a new `ReusableStateCursors` with empty cursor cells.
    pub(crate) const fn new() -> Self {
        Self { plain_storage_state: Cell::new(None) }
    }

    /// Gets a reusable cursor for the `PlainStorageState` table.
    ///
    /// If a cursor is cached, it will be reused. Otherwise, a new cursor is created.
    pub(crate) fn plain_storage_state(
        &self,
        tx: &TX,
    ) -> Result<
        ReusableCursor<'_, PlainStorageState, TX::DupCursor<PlainStorageState>>,
        DatabaseError,
    >
    where
        TX::DupCursor<PlainStorageState>: DbDupCursorRO<PlainStorageState>,
    {
        let cursor =
            self.plain_storage_state.take().map(Ok).unwrap_or_else(|| tx.cursor_dup_read())?;
        Ok(ReusableCursor::new(cursor, &self.plain_storage_state))
    }
}

impl<TX: DbTx> Default for ReusableStateCursors<TX> {
    fn default() -> Self {
        Self::new()
    }
}

/// A Cow-like type for cursor caches that can either own or borrow the cache.
///
/// This allows state providers to either:
/// - Own their cursor cache (standalone `*Ref` construction)
/// - Borrow from an owned wrapper type (via `as_ref()` delegation)
///
/// Uses `Box` for the owned variant to keep the enum pointer-sized, optimizing
/// for the common `Borrowed` case used in macro delegation.
pub(crate) enum CursorCache<'a, TX: DbTx> {
    /// Cursor cache is owned by this instance (boxed to keep enum small).
    Owned(Box<ReusableStateCursors<TX>>),
    /// Cursor cache is borrowed from an owned wrapper.
    Borrowed(&'a ReusableStateCursors<TX>),
}

impl<TX: DbTx> std::fmt::Debug for CursorCache<'_, TX> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Owned(c) => f.debug_tuple("Owned").field(c).finish(),
            Self::Borrowed(c) => f.debug_tuple("Borrowed").field(c).finish(),
        }
    }
}

impl<TX: DbTx> std::ops::Deref for CursorCache<'_, TX> {
    type Target = ReusableStateCursors<TX>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(c) => c.as_ref(),
            Self::Borrowed(c) => c,
        }
    }
}
