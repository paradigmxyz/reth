use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Encode, Table},
    DatabaseError,
};
use std::fmt::Debug;

/// Helper adapter type for accessing [`DbTx`] cursor.
pub type CursorTy<'tx, TX, T> = <TX as DbTx>::Cursor<'tx, T>;

/// Helper adapter type for accessing [`DbTx`] dup cursor.
pub type DupCursorTy<'tx, TX, T> = <TX as DbTx>::DupCursor<'tx, T>;

/// Helper adapter type for accessing [`DbTxMut`] mutable cursor.
pub type CursorMutTy<'tx, TX, T> = <TX as DbTxMut>::CursorMut<'tx, T>;

/// Helper adapter type for accessing [`DbTxMut`] mutable dup cursor.
pub type DupCursorMutTy<'tx, TX, T> = <TX as DbTxMut>::DupCursorMut<'tx, T>;

/// Read only transaction
pub trait DbTx: Debug + Send {
    /// Cursor type for this read-only transaction
    type Cursor<'tx, T: Table>: DbCursorRO<T> + Send
    where
        Self: 'tx;
    /// `DupCursor` type for this read-only transaction
    type DupCursor<'tx, T: DupSort>: DbDupCursorRO<T> + DbCursorRO<T> + Send
    where
        Self: 'tx;

    /// Get value by an owned key
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
    /// Get value by a reference to the encoded key, especially useful for "raw" keys
    /// that encode to themselves like Address and B256. Doesn't need to clone a
    /// reference key like `get`.
    fn get_by_encoded_key<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
    ) -> Result<Option<T::Value>, DatabaseError>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<(), DatabaseError>;
    /// Aborts transaction
    fn abort(self);
    /// Iterate over read only values in table.
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<'_, T>, DatabaseError>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<'_, T>, DatabaseError>;
    /// Returns number of entries in the table.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError>;
    /// Disables long-lived read transaction safety guarantees.
    fn disable_long_read_transaction_safety(&mut self);
}

/// Read write transaction that allows writing to database
pub trait DbTxMut: Send {
    /// Read-Write Cursor type
    type CursorMut<'tx, T: Table>: DbCursorRW<T> + DbCursorRO<T> + Send
    where
        Self: 'tx;
    /// Read-Write `DupCursor` type
    type DupCursorMut<'tx, T: DupSort>: DbDupCursorRW<T>
        + DbCursorRW<T>
        + DbDupCursorRO<T>
        + DbCursorRO<T>
        + Send
    where
        Self: 'tx;

    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    /// Append value with the largest key to database. This should have the same
    /// outcome as `put`, but databases like MDBX provide dedicated modes to make
    /// it much faster, typically from O(logN) down to O(1) thanks to no lookup.
    fn append<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        self.put::<T>(key, value)
    }
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>)
        -> Result<bool, DatabaseError>;
    /// Clears database.
    fn clear<T: Table>(&self) -> Result<(), DatabaseError>;
    /// Cursor mut
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<'_, T>, DatabaseError>;
    /// `DupCursor` mut.
    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<'_, T>, DatabaseError>;
}

/// Read only transaction (unsynchronized variant without Send bounds).
///
/// This is the unsynchronized variant of [`DbTx`] that doesn't require `Send` bounds
/// on the transaction or cursors. It's more efficient for single-threaded use cases.
///
/// Unlike [`DbTx`], this trait uses `&mut self` for all operations because the underlying
/// transaction requires exclusive access for safety.
pub trait DbTxUnsync: Debug {
    /// Cursor type for this read-only transaction
    type Cursor<'tx, T: Table>: DbCursorRO<T>
    where
        Self: 'tx;
    /// `DupCursor` type for this read-only transaction
    type DupCursor<'tx, T: DupSort>: DbDupCursorRO<T> + DbCursorRO<T>
    where
        Self: 'tx;

    /// Get value by an owned key
    fn get<T: Table>(&mut self, key: T::Key) -> Result<Option<T::Value>, DatabaseError>;
    /// Get value by a reference to the encoded key
    fn get_by_encoded_key<T: Table>(
        &mut self,
        key: &<T::Key as Encode>::Encoded,
    ) -> Result<Option<T::Value>, DatabaseError>;
    /// Commit for read only transaction will consume and free transaction
    fn commit(self) -> Result<(), DatabaseError>;
    /// Aborts transaction
    fn abort(self);
    /// Iterate over read only values in table.
    fn cursor_read<T: Table>(&mut self) -> Result<Self::Cursor<'_, T>, DatabaseError>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(&mut self) -> Result<Self::DupCursor<'_, T>, DatabaseError>;
    /// Returns number of entries in the table.
    fn entries<T: Table>(&mut self) -> Result<usize, DatabaseError>;
    /// Disables long-lived read transaction safety guarantees.
    fn disable_long_read_transaction_safety(&mut self);
}

/// Read write transaction (unsynchronized variant without Send bounds).
///
/// This is the unsynchronized variant of [`DbTxMut`] that doesn't require `Send` bounds
/// on the transaction or cursors. It's more efficient for single-threaded use cases.
///
/// Unlike [`DbTxMut`], this trait uses `&mut self` for all operations because the underlying
/// transaction requires exclusive access for safety.
pub trait DbTxMutUnsync {
    /// Read-Write Cursor type
    type CursorMut<'tx, T: Table>: DbCursorRW<T> + DbCursorRO<T>
    where
        Self: 'tx;
    /// Read-Write `DupCursor` type
    type DupCursorMut<'tx, T: DupSort>: DbDupCursorRW<T>
        + DbCursorRW<T>
        + DbDupCursorRO<T>
        + DbCursorRO<T>
    where
        Self: 'tx;

    /// Put value to database
    fn put<T: Table>(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
    /// Append value with the largest key to database
    fn append<T: Table>(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        self.put::<T>(key, value)
    }
    /// Delete value from database
    fn delete<T: Table>(
        &mut self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, DatabaseError>;
    /// Clears database.
    fn clear<T: Table>(&mut self) -> Result<(), DatabaseError>;
    /// Cursor mut
    fn cursor_write<T: Table>(&mut self) -> Result<Self::CursorMut<'_, T>, DatabaseError>;
    /// `DupCursor` mut.
    fn cursor_dup_write<T: DupSort>(&mut self) -> Result<Self::DupCursorMut<'_, T>, DatabaseError>;
}
