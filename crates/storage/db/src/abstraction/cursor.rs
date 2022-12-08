use std::marker::PhantomData;

use crate::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    table::{DupSort, Table},
    Error,
};

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

    /// Database operation that will insert a row at a given key. If the key is already
    /// present, the operation will result in an error.
    fn insert(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;

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
    pub start: IterPairResult<T>,
    /// Phantom data for 'tx. As it is only used for `DbDupCursorRO`.
    pub _tx_phantom: PhantomData<&'tx T>,
}

impl<'cursor, 'tx, T: DupSort, CURSOR: DbDupCursorRO<'tx, T>> std::iter::Iterator
    for DupWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<(T::Key, T::Value), Error>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup().transpose()
    }
}
