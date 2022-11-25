pub mod codecs;
mod error;
pub mod mock;
pub mod models;
mod table;
pub mod tables;

use std::marker::PhantomData;

pub use error::Error;
pub use table::*;

// Sealed trait helper to prevent misuse of the API.
mod sealed {
    pub trait Sealed: Sized {}
    pub struct Bounds<T>(T);
    impl<T> Sealed for Bounds<T> {}
}
use sealed::{Bounds, Sealed};

/// Implements the GAT method from:
/// https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DatabaseGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// RO database transaction
    type TX: DbTx<'a> + Send + Sync;
    /// RW database transaction
    type TXMut: DbTxMut<'a> + DbTx<'a> + Send + Sync;
}

/// Main Database trait that spawns transactions to be executed.
pub trait Database: for<'a> DatabaseGAT<'a> {
    /// Create read only transaction.
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, Error>;

    /// Create read write transaction only possible if database is open with write access.
    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, Error>;

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    fn view<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: Fn(&<Self as DatabaseGAT<'_>>::TX) -> T,
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
        F: Fn(&<Self as DatabaseGAT<'_>>::TXMut) -> T,
    {
        let tx = self.tx_mut()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

/// Implements the GAT method from:
/// https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats.
///
/// Sealed trait which cannot be implemented by 3rd parties, exposed only for implementers
pub trait DbTxGAT<'a, __ImplicitBounds: Sealed = Bounds<&'a Self>>: Send + Sync {
    /// Cursor GAT
    type Cursor<T: Table>: DbCursorRO<'a, T> + Send + Sync;
    /// DupCursor GAT
    type DupCursor<T: DupSort>: DbDupCursorRO<'a, T> + DbCursorRO<'a, T> + Send + Sync;
}

/// Implements the GAT method from:
/// https://sabrinajewson.org/blog/the-better-alternative-to-lifetime-gats#the-better-gats.
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
    fn cursor<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error>;
    /// Iterate over read only values in dup sorted table.
    fn cursor_dup<T: DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error>;
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
    fn cursor_mut<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error>;
    /// DupCursor mut.
    fn cursor_dup_mut<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error>;
}

/// Alias type for a `(key, value)` result coming from a cursor.
pub type PairResult<T> = Result<Option<(<T as Table>::Key, <T as Table>::Value)>, Error>;
/// Alias type for a `(key, value)` result coming from an iterator.
pub type IterPairResult<T> = Option<Result<(<T as Table>::Key, <T as Table>::Value), Error>>;
/// Alias type for a value result coming from a cursor without its key.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, Error>;

/// Read only cursor over table.
pub trait DbCursorRO<'tx, T: Table> {
    /// First item in table
    fn first(&mut self) -> PairResult<T>;

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
    fn walk<'cursor>(
        &'cursor mut self,
        start_key: T::Key,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized;
}

/// Read only curor over DupSort table.
pub trait DbDupCursorRO<'tx, T: DupSort> {
    /// Seeks for a `(key, value)` pair greater or equal than `key`.
    fn seek(&mut self, key: T::SubKey) -> PairResult<T>;

    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T>;

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T>;

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T>;

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DUPSORT
    /// table.
    fn walk_dup<'cursor>(
        &'cursor mut self,
        key: T::Key,
        subkey: T::SubKey,
    ) -> Result<DupWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized;
}

/// Read write cursor over table.
pub trait DbCursorRW<'tx, T: Table> {
    /// Database operation that will update an existing row if a specified value already
    /// exists in a table, and insert a new row if the specified value doesn't already exist
    fn upsert(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;

    /// Append value to next cursor item.
    ///
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use [`insert`].
    fn append(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;

    /// Delete current value that cursor points to
    fn delete_current(&mut self) -> Result<(), Error>;
}

/// Read Write Cursor over DupSorted table.
pub trait DbDupCursorRW<'tx, T: DupSort> {
    /// Append value to next cursor item
    fn delete_current_duplicates(&mut self) -> Result<(), Error>;

    /// Append duplicate value.
    ///
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use [`insert`].
    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;
}

/// Provides an iterator to `Cursor` when handling `Table`.
///
/// Reason why we have two lifetimes is to distinguish between `'cursor` lifetime
/// and inherited `'tx` lifetime. If there is only one, rust would short circle
/// the Cursor lifetime and it wouldn't be possible to use Walker.
pub struct Walker<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> {
    /// Cursor to be used to walk through the table.
    pub cursor: &'cursor mut CURSOR,
    /// `(key, value)` where to start the walk.
    pub start: IterPairResult<T>,
    /// Phantom data for 'tx. As it is only used for `DbCursorRO`.
    pub _tx_phantom: PhantomData<&'tx T>,
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for Walker<'cursor, 'tx, T, CURSOR>
{
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
///
/// Reason why we have two lifetimes is to distinguish between `'cursor` lifetime
/// and inherited `'tx` lifetime. If there is only one, rust would short circle
/// the Cursor lifetime and it wouldn't be possible to use Walker.
pub struct DupWalker<'cursor, 'tx, T: DupSort, CURSOR: DbDupCursorRO<'tx, T>> {
    /// Cursor to be used to walk through the table.
    pub cursor: &'cursor mut CURSOR,
    /// Value where to start the walk.
    pub start: Option<Result<T::Value, Error>>,
    /// Phantom data for 'tx. As it is only used for `DbDupCursorRO`.
    pub _tx_phantom: PhantomData<&'tx T>,
}

impl<'cursor, 'tx, T: DupSort, CURSOR: DbDupCursorRO<'tx, T>> std::iter::Iterator
    for DupWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<T::Value, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup_val().transpose()
    }
}

#[macro_export]
/// Implements the [`arbitrary::Arbitrary`] trait for types with fixed array types.
macro_rules! impl_fixed_arbitrary {
    ($name:tt, $size:tt) => {
        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::{Arbitrary, Unstructured};

        #[cfg(any(test, feature = "arbitrary"))]
        impl<'a> Arbitrary<'a> for $name {
            fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, arbitrary::Error> {
                let mut buffer = vec![0; $size];
                u.fill_buffer(buffer.as_mut_slice())?;

                Decode::decode(buffer).map_err(|_| arbitrary::Error::IncorrectFormat)
            }
        }
    };
}
