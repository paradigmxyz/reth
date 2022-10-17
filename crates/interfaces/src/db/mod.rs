mod error;
mod table;
pub mod tables;

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
}

/// Read only transaction
pub trait DbTx<'a> {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T>
    where
        Self: 'a;
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool, Error>;
    /// Iterate over read only values in database.
    fn cursor<T: Table>(&self) -> Result<Self::Cursor<T>, Error>
    where
        <T as Table>::Key: Decode;
    /// Get value
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error>;
}

/// Read write transaction that allows writing to database
pub trait DbTxMut<'a>: DbTx<'a> {
    /// Put value to database
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error>;
    /// Delete value from database
    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>;
    //fn cursor_mut(&self) -> Cursor<'a, RW, T>;
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
    fn walk<IT: Iterator<Item = Result<(<T as Table>::Key, <T as Table>::Value), Error>>>(
        &mut self,
        start_key: T::Key,
    ) -> Result<IT, Error>;
}
