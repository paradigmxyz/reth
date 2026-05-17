use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, Encode, Table},
    DatabaseError,
};
use std::fmt::Debug;

/// Database page-operation counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DatabasePageOps {
    /// Quantity of new pages added.
    pub newly: u64,
    /// Quantity of pages copied for update.
    pub cow: u64,
    /// Quantity of parent's dirty pages clones for nested transactions.
    pub clone: u64,
    /// Page splits.
    pub split: u64,
    /// Page merges.
    pub merge: u64,
    /// Quantity of spilled dirty pages.
    pub spill: u64,
    /// Quantity of unspilled/reloaded pages.
    pub unspill: u64,
    /// Number of explicit write operations to disk.
    pub wops: u64,
    /// Number of prefault write operations.
    pub prefault: u64,
    /// Number of `mincore()` calls.
    pub mincore: u64,
    /// Number of explicit msync operations.
    pub msync: u64,
    /// Number of explicit fsync operations.
    pub fsync: u64,
}

impl DatabasePageOps {
    /// Returns a saturating field-wise difference.
    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self {
            newly: self.newly.saturating_sub(rhs.newly),
            cow: self.cow.saturating_sub(rhs.cow),
            clone: self.clone.saturating_sub(rhs.clone),
            split: self.split.saturating_sub(rhs.split),
            merge: self.merge.saturating_sub(rhs.merge),
            spill: self.spill.saturating_sub(rhs.spill),
            unspill: self.unspill.saturating_sub(rhs.unspill),
            wops: self.wops.saturating_sub(rhs.wops),
            prefault: self.prefault.saturating_sub(rhs.prefault),
            mincore: self.mincore.saturating_sub(rhs.mincore),
            msync: self.msync.saturating_sub(rhs.msync),
            fsync: self.fsync.saturating_sub(rhs.fsync),
        }
    }
}

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
    /// Returns database page-operation counters if the backend exposes them.
    fn page_ops(&self) -> Result<Option<DatabasePageOps>, DatabaseError> {
        Ok(None)
    }
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
}
