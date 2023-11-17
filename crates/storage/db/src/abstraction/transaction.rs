use crate::{
    common::{Sealed},
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Table},
    DatabaseError,
};

/// Trait that provides the different cursor types.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DbTxGAT : Send + Sync {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<T> + Send + Sync;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<T> + DbCursorRO<T> + Send + Sync;
}

/// Implements the GAT method from:
/// <https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats>.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DbTxMutGAT: Send + Sync {
    /// Cursor GAT
    type CursorMut<T: Table>: DbCursorRW<T> + DbCursorRO<T> + Send + Sync;
    /// DupCursor GAT
    type DupCursorMut<T: DupSort>: DbDupCursorRW<T>
        + DbCursorRW<T>
        + DbDupCursorRO<T>
        + DbCursorRO<T>
        + Send
        + Sync;
}

/// Read only transaction
pub trait DbTx:  DbTxGAT {
    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, DatabaseError>;
    /// Aborts transaction
    fn abort(self);
    /// Iterate over read only values in table.
    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT>::Cursor<T>, DatabaseError>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxGAT>::DupCursor<T>, DatabaseError>;
    /// Returns number of entries in the table.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError>;
}

/// Read write transaction that allows writing to database
pub trait DbTxMut: DbTxMutGAT {
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>)
        -> Result<bool, DatabaseError>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), DatabaseError>;
    /// Cursor mut
    fn cursor_write<T: Table>(
        &self,
    ) -> Result<<Self as DbTxMutGAT>::CursorMut<T>, DatabaseError>;
    /// DupCursor mut.
    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT>::DupCursorMut<T>, DatabaseError>;
}
