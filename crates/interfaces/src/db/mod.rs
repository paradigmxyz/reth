mod error;
mod table;
pub mod tables;
pub mod models;


pub use error::Error;
pub use table::*;

/// Main Database trait that spawns transactions to be executed.
pub trait Database {
    /// RO database transaction
    type TX<'a>: DbTx<'a>
    where
        Self: 'a;
    /// RW database transaction
    type TXMut<'a>: DbTxMut<'a>
    where
        Self: 'a;
    /// Create read only transaction.
    fn tx<'a>(&'a self) -> Result<Self::TX<'a>, Error>;
    /// Create read write transaction only possible if database is open with write access.
    fn tx_mut<'a>(&'a self) -> Result<Self::TXMut<'a>, Error>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: Fn(&Self::TX<'_>) -> T,
    {
        let tx = self.tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    fn update<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: Fn(&Self::TXMut<'_>) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

/// Read only transaction
pub trait DbTx<'a> {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T>;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<'a, T>;
    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, Error>;
    /// Iterate over read only values in table.
    fn cursor<T: Table>(&self) -> Result<Self::Cursor<T>, Error>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup<T: DupSort>(&self) -> Result<Self::DupCursor<T>, Error>;
}

/// Read write transaction that allows writing to database
pub trait DbTxMut<'a>: DbTx<'a> {
    /// Cursor GAT
    type CursorMut<T: Table>: DbCursorRW<'a, T>;
    /// DupCursor GAT
    type DupCursorMut<T: DupSort>: DbDupCursorRW<'a, T>;
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>;
    /// Cursor mut
    fn cursor_mut<T: Table>(&self) -> Result<Self::CursorMut<T>, Error>;
    /// DupCursor mut.
    /// NOTE: check if this is needed.
    fn cursor_dup_mut<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, Error>;
}

/// Alias type for a `(key, value)` result coming from a cursor.
pub type PairResult<T> = Result<Option<(<T as Table>::Key, <T as Table>::Value)>, Error>;
/// Alias type for a `(key, value)` result coming from an iterator.
pub type IterPairResult<T> = Option<Result<(<T as Table>::Key, <T as Table>::Value), Error>>;
/// Alias type for a value result coming from a cursor without its key.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, Error>;

/// Read only cursor over table
pub trait DbCursorRO<'tx, T: Table> {
    /// First item in table
    fn first(&mut self) -> PairResult<T>;

    /// Seeks for a `(key, value)` pair greater or equal than `key`.
    fn seek(&mut self, key: T::SeekKey) -> PairResult<T>;

    /// Seeks for the exact `(key, value)` pair with `key`.
    fn seek_exact(&mut self, key: T::Key) -> PairResult<T>;

    /// Returns the next `(key, value)` pair.
    #[allow(clippy::should_implement_trait)]
    fn next(&mut self) -> PairResult<T>;

    /// Returns the previous `(key, value)` pair.
    fn prev(&mut self) -> PairResult<T>;

    /// Returns the last `(key, value)` pair.
    fn last(&mut self) -> PairResult<T>;

    /// Returns the current `(key, value)` pair of the cursor.
    fn current(&mut self) -> PairResult<T>;

    /// Returns an iterator starting at a key greater or equal than `start_key`.
    fn walk(&'tx mut self, start_key: T::Key) -> Result<Walker<'tx, T>, Error>;
}

/// Read only cursor over table
pub trait DbCursorRW<'tx, T: Table>: DbCursorRO<'tx, T> {
    /// Put change.
    /// TODO implement write flags
    fn put(
        &mut self,
        k: T::Key,
        v: T::Value, /* , f: Option<WriteFlags> */
    ) -> Result<(), Error>;
}

/// DupSort Transaction
pub trait DbDupCursorRO<'tx, T: DupSort>: DbCursorRO<'tx, T> {
    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T>;

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T>;

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T>;

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DUPSORT
    /// table.
    fn walk_dup(&'tx mut self, key: T::Key, subkey: T::SubKey) -> Result<DupWalker<'tx, T>, Error>;
}

/// Read Write Cursor over DupSorted table
pub trait DbDupCursorRW<'tx, T: DupSort>: DbCursorRO<'tx, T> {
    //
    /// TODO implement write flags
    fn put(
        &mut self,
        k: T::Key,
        v: T::Value, /* , f: Option<WriteFlags> */
    ) -> Result<(), Error>;
}

/// Provides an iterator to `Cursor` when handling `Table`.
pub struct Walker<'cursor, T: Table> {
    /// Cursor to be used to walk through the table.
    pub cursor: &'cursor mut dyn DbCursorRO<'cursor, T>,
    /// `(key, value)` where to start the walk.
    pub start: IterPairResult<T>,
}

impl<'cursor, T: Table> std::iter::Iterator for Walker<'cursor, T> {
    type Item = Result<(T::Key, T::Value), Error>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }

        self.cursor.next().transpose()
    }
}

/// Provides an iterator to `Cursor` when handling a `DupSort` table.
pub struct DupWalker<'cursor, T: DupSort> {
    /// Cursor to be used to walk through the table.
    pub cursor: &'cursor mut dyn DbDupCursorRO<'cursor, T>,
    /// Value where to start the walk.
    pub start: Option<Result<T::Value, Error>>,
}

impl<'cursor, T: DupSort> std::iter::Iterator for DupWalker<'cursor, T> {
    type Item = Result<T::Value, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup_val().transpose()
    }
}
