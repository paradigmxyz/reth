//! Mock database implementation for testing and development.
//!
//! Provides lightweight mock implementations of database traits. All operations
//! are no-ops that return default values without persisting data.

use crate::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    database::Database,
    database_metrics::DatabaseMetrics,
    table::{DupSort, Encode, Table, TableImporter},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use core::ops::Bound;
use std::{collections::BTreeMap, ops::RangeBounds};

/// Mock database implementation for testing and development.
///
/// Provides a lightweight implementation of the [`Database`] trait suitable
/// for testing scenarios where actual database operations are not required.
#[derive(Clone, Debug, Default)]
pub struct DatabaseMock {
    /// Internal data storage using a `BTreeMap`.
    ///
    /// TODO: Make the mock database table-aware by properly utilizing
    /// this data structure to simulate realistic database behavior during testing.
    pub data: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Database for DatabaseMock {
    type TX = TxMock;
    type TXMut = TxMock;

    /// Creates a new read-only transaction.
    ///
    /// This always succeeds and returns a default [`TxMock`] instance.
    /// The mock transaction doesn't actually perform any database operations.
    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        Ok(TxMock::default())
    }

    /// Creates a new read-write transaction.
    ///
    /// This always succeeds and returns a default [`TxMock`] instance.
    /// The mock transaction doesn't actually perform any database operations.
    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        Ok(TxMock::default())
    }
}

impl DatabaseMetrics for DatabaseMock {}

/// Mock transaction implementation for testing and development.
///
/// Implements both [`DbTx`] and [`DbTxMut`] traits. All operations are no-ops
/// that return success or default values, suitable for testing database operations
/// without side effects.
#[derive(Debug, Clone, Default)]
pub struct TxMock {
    /// Internal table representation (currently unused).
    _table: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl DbTx for TxMock {
    type Cursor<T: Table> = CursorMock;
    type DupCursor<T: DupSort> = CursorMock;

    /// Retrieves a value by key from the specified table.
    ///
    /// **Mock behavior**: Always returns `None` regardless of the key.
    /// This simulates a table with no data, which is typical for testing
    /// scenarios where you want to verify that read operations are called
    /// correctly without actually storing data.
    fn get<T: Table>(&self, _key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        Ok(None)
    }

    /// Retrieves a value by encoded key from the specified table.
    ///
    /// **Mock behavior**: Always returns `None` regardless of the encoded key.
    /// This is equivalent to [`Self::get`] but works with pre-encoded keys.
    fn get_by_encoded_key<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
    ) -> Result<Option<T::Value>, DatabaseError> {
        Ok(None)
    }

    /// Commits the transaction.
    ///
    /// **Mock behavior**: Always returns `Ok(true)`, indicating successful commit.
    /// No actual data is persisted since this is a mock implementation.
    fn commit(self) -> Result<bool, DatabaseError> {
        Ok(true)
    }

    /// Aborts the transaction.
    ///
    /// **Mock behavior**: No-op. Since no data is actually stored in the mock,
    /// there's nothing to rollback.
    fn abort(self) {}

    /// Creates a read-only cursor for the specified table.
    ///
    /// **Mock behavior**: Returns a default [`CursorMock`] that will not
    /// iterate over any data (all cursor operations return `None`).
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError> {
        Ok(CursorMock { _cursor: 0 })
    }

    /// Creates a read-only duplicate cursor for the specified duplicate sort table.
    ///
    /// **Mock behavior**: Returns a default [`CursorMock`] that will not
    /// iterate over any data (all cursor operations return `None`).
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError> {
        Ok(CursorMock { _cursor: 0 })
    }

    /// Returns the number of entries in the specified table.
    ///
    /// **Mock behavior**: Returns the length of the internal `_table` `BTreeMap`,
    /// which is typically 0 since no data is actually stored.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self._table.len())
    }

    /// Disables long read transaction safety checks.
    ///
    /// **Mock behavior**: No-op. This is a performance optimization that
    /// doesn't apply to the mock implementation.
    fn disable_long_read_transaction_safety(&mut self) {}
}

impl DbTxMut for TxMock {
    type CursorMut<T: Table> = CursorMock;
    type DupCursorMut<T: DupSort> = CursorMock;

    /// Inserts or updates a key-value pair in the specified table.
    ///
    /// **Mock behavior**: Always returns `Ok(())` without actually storing
    /// the data. This allows tests to verify that write operations are called
    /// correctly without side effects.
    fn put<T: Table>(&self, _key: T::Key, _value: T::Value) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Deletes a key-value pair from the specified table.
    ///
    /// **Mock behavior**: Always returns `Ok(true)`, indicating successful
    /// deletion, without actually removing any data.
    fn delete<T: Table>(
        &self,
        _key: T::Key,
        _value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        Ok(true)
    }

    /// Clears all entries from the specified table.
    ///
    /// **Mock behavior**: Always returns `Ok(())` without actually clearing
    /// any data. This simulates successful table clearing for testing purposes.
    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Creates a write cursor for the specified table.
    ///
    /// **Mock behavior**: Returns a default [`CursorMock`] that will not
    /// iterate over any data and all write operations will be no-ops.
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError> {
        Ok(CursorMock { _cursor: 0 })
    }

    /// Creates a write duplicate cursor for the specified duplicate sort table.
    ///
    /// **Mock behavior**: Returns a default [`CursorMock`] that will not
    /// iterate over any data and all write operations will be no-ops.
    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError> {
        Ok(CursorMock { _cursor: 0 })
    }
}

impl TableImporter for TxMock {}

/// Mock cursor implementation for testing and development.
///
/// Implements all cursor traits. All operations are no-ops that return empty
/// results, suitable for testing cursor operations without side effects.
#[derive(Debug)]
pub struct CursorMock {
    /// Internal cursor position (currently unused).
    _cursor: u32,
}

impl<T: Table> DbCursorRO<T> for CursorMock {
    /// Moves to the first entry in the table.
    /// **Mock behavior**: Always returns `None`.
    fn first(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Seeks to an exact key match.
    /// **Mock behavior**: Always returns `None`.
    fn seek_exact(&mut self, _key: T::Key) -> PairResult<T> {
        Ok(None)
    }

    /// Seeks to the first key greater than or equal to the given key.
    /// **Mock behavior**: Always returns `None`.
    fn seek(&mut self, _key: T::Key) -> PairResult<T> {
        Ok(None)
    }

    /// Moves to the next entry.
    /// **Mock behavior**: Always returns `None`.
    fn next(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Moves to the previous entry.
    /// **Mock behavior**: Always returns `None`.
    fn prev(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Moves to the last entry in the table.
    /// **Mock behavior**: Always returns `None`.
    fn last(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Returns the current entry without moving the cursor.
    /// **Mock behavior**: Always returns `None`.
    fn current(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Creates a forward walker starting from the given key.
    /// **Mock behavior**: Returns an empty walker that won't iterate over any data.
    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match start_key {
            Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
            None => <Self as DbCursorRO<T>>::first(self).transpose(),
        };

        Ok(Walker::new(self, start))
    }

    /// Creates a range walker for the specified key range.
    /// **Mock behavior**: Returns an empty walker that won't iterate over any data.
    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'_, T, Self>, DatabaseError> {
        let start_key = match range.start_bound() {
            Bound::Included(key) | Bound::Excluded(key) => Some((*key).clone()),
            Bound::Unbounded => None,
        };

        let end_key = match range.end_bound() {
            Bound::Included(key) | Bound::Excluded(key) => Bound::Included((*key).clone()),
            Bound::Unbounded => Bound::Unbounded,
        };

        let start: IterPairResult<T> = match start_key {
            Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
            None => <Self as DbCursorRO<T>>::first(self).transpose(),
        };

        Ok(RangeWalker::new(self, start, end_key))
    }

    /// Creates a backward walker starting from the given key.
    /// **Mock behavior**: Returns an empty walker that won't iterate over any data.
    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match start_key {
            Some(key) => <Self as DbCursorRO<T>>::seek(self, key).transpose(),
            None => <Self as DbCursorRO<T>>::last(self).transpose(),
        };
        Ok(ReverseWalker::new(self, start))
    }
}

impl<T: DupSort> DbDupCursorRO<T> for CursorMock {
    /// Moves to the next duplicate entry.
    /// **Mock behavior**: Always returns `None`.
    fn next_dup(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Moves to the next entry with a different key.
    /// **Mock behavior**: Always returns `None`.
    fn next_no_dup(&mut self) -> PairResult<T> {
        Ok(None)
    }

    /// Moves to the next duplicate value.
    /// **Mock behavior**: Always returns `None`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        Ok(None)
    }

    /// Seeks to a specific key-subkey combination.
    /// **Mock behavior**: Always returns `None`.
    fn seek_by_key_subkey(
        &mut self,
        _key: <T as Table>::Key,
        _subkey: <T as DupSort>::SubKey,
    ) -> ValueOnlyResult<T> {
        Ok(None)
    }

    /// Creates a duplicate walker for the specified key and subkey.
    /// **Mock behavior**: Returns an empty walker that won't iterate over any data.
    fn walk_dup(
        &mut self,
        _key: Option<<T>::Key>,
        _subkey: Option<<T as DupSort>::SubKey>,
    ) -> Result<DupWalker<'_, T, Self>, DatabaseError> {
        Ok(DupWalker { cursor: self, start: None })
    }
}

impl<T: Table> DbCursorRW<T> for CursorMock {
    /// Inserts or updates a key-value pair at the current cursor position.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn upsert(
        &mut self,
        _key: <T as Table>::Key,
        _value: &<T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Inserts a key-value pair at the current cursor position.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn insert(
        &mut self,
        _key: <T as Table>::Key,
        _value: &<T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Appends a key-value pair at the end of the table.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn append(
        &mut self,
        _key: <T as Table>::Key,
        _value: &<T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Deletes the entry at the current cursor position.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        Ok(())
    }
}

impl<T: DupSort> DbDupCursorRW<T> for CursorMock {
    /// Deletes all duplicate entries at the current cursor position.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Appends a duplicate key-value pair.
    /// **Mock behavior**: Always succeeds without modifying any data.
    fn append_dup(&mut self, _key: <T>::Key, _value: <T>::Value) -> Result<(), DatabaseError> {
        Ok(())
    }
}
