mod error;
mod table;
pub mod tables;

pub use error::Error;
pub use table::*;

/// Main Database trait that spawns transactions to be executed.
/// NOTE: we could introduce generic DbTx and DbTxMut to
/// remove dynamic dispatch, but it seems unnecesarry.
pub trait Database {
    /// Create read only transaction.
    fn tx<'a, T: Table>(&'a self) -> Result<Box<dyn DbTx<'a, T> + 'a>,Error>;
    /// Create read write transaction only possible if database is open with write access.
    fn tx_mut<'a, T: Table>(&'a self) -> Result<Box<dyn DbTxMut<'a, T> + 'a>,Error>;
}

/// Read only transaction
pub trait DbTx<'a, T: Table> {
    /// Commit for read only transaction will consume and free transaction and allows
    /// freeing of memory pages
    fn commit(self) -> Result<bool,Error>;
    //fn cursor(&'a self) -> Cursor<'a, RO, T>;
    /// Get value
    fn get(&self, key: T::Key) -> Result<Option<T::Value>,Error>;
}

/// Read write transaction that allows writing to database
pub trait DbTxMut<'a, T: Table>: DbTx<'a, T> {
    /// Put value to database
    fn put(&self, key: T::Key, value: T::Value) -> Result<(),Error>;
    /// Delete value from database
    fn delete(&self, key: T::Key, value: Option<T::Value>) -> Result<bool,Error>;
    //fn cursor_mut(&self) -> Cursor<'a, RW, T>;
}
