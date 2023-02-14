use crate::{
    common::{Bounds, Sealed},
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Table},
    Error,
};

/// Implements the GAT method from:
/// <https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats>.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DbTxGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T> + Send + Sync;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<'a, T> + DbCursorRO<'a, T> + Send + Sync;
}

/// Implements the GAT method from:
/// <https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats>.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DbTxMutGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// Cursor GAT
    type CursorMut<T: Table>: DbCursorRW<'a, T> + DbCursorRO<'a, T> + Send + Sync;
    /// DupCursor GAT
    type DupCursorMut<T: DupSort>: DbDupCursorRW<'a, T>
        + DbCursorRW<'a, T>
        + DbDupCursorRO<'a, T>
        + DbCursorRO<'a, T>
        + Send
        + Sync;
}

/// Read only transaction
pub trait DbTx<'tx>: for<'a> DbTxGAT<'a> {
    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, Error>;
    /// Iterate over read only values in table.
    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error>;
}

/// Read write transaction that allows writing to database
pub trait DbTxMut<'tx>: for<'a> DbTxMutGAT<'a> {
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), Error>;
    /// Cursor mut
    fn cursor_write<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error>;
    /// DupCursor mut.
    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error>;
}
