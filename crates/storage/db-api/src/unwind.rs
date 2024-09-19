use crate::{cursor::DbCursorRO, table::Table, transaction::DbTxMut};
use reth_storage_errors::db::DatabaseError;
use std::ops::RangeBounds;

/// Extension trait for [`DbTxMut`] that provides unwind functionality.
pub trait DbTxUnwindExt: DbTxMut {
    /// Unwind table by some number key.
    /// Returns number of rows unwound.
    ///
    /// Note: Key is not inclusive and specified key would stay in db.
    #[inline]
    fn unwind_table_by_num<T>(&self, num: u64) -> Result<usize, DatabaseError>
    where
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind the table to a provided number key.
    /// Returns number of rows unwound.
    ///
    /// Note: Key is not inclusive and specified key would stay in db.
    fn unwind_table<T, F>(&self, key: u64, mut selector: F) -> Result<usize, DatabaseError>
    where
        T: Table,
        F: FnMut(T::Key) -> u64,
    {
        let mut cursor = self.cursor_write::<T>()?;
        let mut reverse_walker = cursor.walk_back(None)?;
        let mut deleted = 0;

        while let Some(Ok((entry_key, _))) = reverse_walker.next() {
            if selector(entry_key.clone()) <= key {
                break
            }
            reverse_walker.delete_current()?;
            deleted += 1;
        }

        Ok(deleted)
    }

    /// Unwind a table forward by a [`Walker`][crate::cursor::Walker] on another table.
    ///
    /// Note: Range is inclusive and first key in the range is removed.
    fn unwind_table_by_walker<T1, T2>(
        &self,
        range: impl RangeBounds<T1::Key>,
    ) -> Result<(), DatabaseError>
    where
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = self.cursor_write::<T1>()?;
        let mut walker = cursor.walk_range(range)?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.delete::<T2>(value, None)?;
        }
        Ok(())
    }
}

impl<T> DbTxUnwindExt for T where T: DbTxMut {}
