use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Encode, Table},
    DatabaseError,
};
use std::fmt::Debug;

/// Helper adapter type for accessing [`DbTx`] cursor.
pub type CursorTy<TX, T> = <TX as DbTx>::Cursor<T>;

/// Helper adapter type for accessing [`DbTx`] dup cursor.
pub type DupCursorTy<TX, T> = <TX as DbTx>::DupCursor<T>;

/// Helper adapter type for accessing [`DbTxMut`] mutable cursor.
pub type CursorMutTy<TX, T> = <TX as DbTxMut>::CursorMut<T>;

/// Helper adapter type for accessing [`DbTxMut`] mutable dup cursor.
pub type DupCursorMutTy<TX, T> = <TX as DbTxMut>::DupCursorMut<T>;

/// Read only transaction
pub trait DbTx: Debug + Send {
    /// Cursor type for this read-only transaction
    type Cursor<T: Table>: DbCursorRO<T> + Send;
    /// `DupCursor` type for this read-only transaction
    type DupCursor<T: DupSort>: DbDupCursorRO<T> + DbCursorRO<T> + Send;

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
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError>;
    /// Returns number of entries in the table.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError>;
    /// Disables long-lived read transaction safety guarantees.
    fn disable_long_read_transaction_safety(&mut self);
}

/// Read write transaction that allows writing to database
pub trait DbTxMut: Send {
    /// Read-Write Cursor type
    type CursorMut<T: Table>: DbCursorRW<T> + DbCursorRO<T> + Send;
    /// Read-Write `DupCursor` type
    type DupCursorMut<T: DupSort>: DbDupCursorRW<T>
        + DbCursorRW<T>
        + DbDupCursorRO<T>
        + DbCursorRO<T>
        + Send;

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
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError>;
    /// `DupCursor` mut.
    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError>;

    /// Enables parallel writes mode, allowing multiple threads to write to different tables
    /// simultaneously. Must be called before any parallel cursor operations.
    fn enable_parallel_writes(&self) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Returns whether parallel writes mode is currently enabled.
    fn is_parallel_writes_enabled(&self) -> bool {
        false
    }

    /// Commits all sub-transactions created during parallel writes.
    fn commit_subtxns(&self) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Enables parallel writes mode only for the specified tables.
    ///
    /// This creates subtransactions only for the listed tables, allowing parallel
    /// writes to those tables while other tables continue using the main transaction.
    fn enable_parallel_writes_for_tables(&self, tables: &[&str]) -> Result<(), DatabaseError> {
        let hints: Vec<_> = tables.iter().map(|&t| (t, 0usize)).collect();
        self.enable_parallel_writes_for_tables_with_hints(&hints)
    }

    /// Enables parallel writes mode only for the specified tables with arena size hints.
    ///
    /// Similar to [`enable_parallel_writes_for_tables`], but allows specifying an arena_hint
    /// for each table to guide page pre-allocation. An arena_hint of 0 means use
    /// equal distribution among all subtransactions.
    ///
    /// # Arguments
    /// * `tables` - Slice of (table_name, arena_hint) tuples.
    fn enable_parallel_writes_for_tables_with_hints(
        &self,
        _tables: &[(&str, usize)],
    ) -> Result<(), DatabaseError> {
        Ok(())
    }
}
