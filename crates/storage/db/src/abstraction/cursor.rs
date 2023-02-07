use std::{marker::PhantomData, ops::Range};

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

    /// Seeks for a `(key, value)` pair greater or equal than `key`.
    fn seek(&mut self, key: T::Key) -> PairResult<T>;

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

    /// Returns an iterator starting at a key greater or equal than `start_key` and ending at a key
    /// less than `end_key`
    fn walk_range<'cursor>(
        &'cursor mut self,
        range: Range<T::Key>,
    ) -> Result<RangeWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized;

    /// Returns an iterator that walks backwards through the table. If `start_key`
    /// is None, starts from the last entry of the table. If it not, starts at a key
    /// greater or equal than the key value wrapped inside Some().
    fn walk_back<'cursor>(
        &'cursor mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized;
}

/// Read only cursor over DupSort table.
pub trait DbDupCursorRO<'tx, T: DupSort> {
    /// Returns the next `(key, value)` pair of a DupSort table.
    fn next_dup(&mut self) -> PairResult<T>;

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T>;

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T>;

    /// Seek by key and subkey
    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T>;

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DupSort
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
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use
    /// [`DbCursorRW::insert`].
    fn append(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;

    /// Delete current value that cursor points to
    fn delete_current(&mut self) -> Result<(), Error>;
}

/// Read Write Cursor over DupSorted table.
pub trait DbDupCursorRW<'tx, T: DupSort> {
    /// Delete all duplicate entries for current key.
    fn delete_current_duplicates(&mut self) -> Result<(), Error>;

    /// Append duplicate value.
    ///
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use `insert`.
    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), Error>;
}

/// Provides an iterator to `Cursor` when handling `Table`.
///
/// Reason why we have two lifetimes is to distinguish between `'cursor` lifetime
/// and inherited `'tx` lifetime. If there is only one, rust would short circle
/// the Cursor lifetime and it wouldn't be possible to use Walker.
pub struct Walker<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> {
    /// Cursor to be used to walk through the table.
    cursor: &'cursor mut CURSOR,
    /// `(key, value)` where to start the walk.
    start: IterPairResult<T>,
    /// Phantom data for 'tx. As it is only used for `DbCursorRO`.
    _tx_phantom: PhantomData<&'tx T>,
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

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> Walker<'cursor, 'tx, T, CURSOR> {
    /// construct Walker
    pub fn new(cursor: &'cursor mut CURSOR, start: IterPairResult<T>) -> Self {
        Self { cursor, start, _tx_phantom: std::marker::PhantomData }
    }

    /// convert current [`Walker`] to [`ReverseWalker`] which iterates reversely
    pub fn rev(self) -> ReverseWalker<'cursor, 'tx, T, CURSOR> {
        let start = self.cursor.current().transpose();
        ReverseWalker::new(self.cursor, start)
    }
}

/// Provides a reverse iterator to `Cursor` when handling `Table`.
/// Also check [`Walker`]
pub struct ReverseWalker<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> {
    /// Cursor to be used to walk through the table.
    cursor: &'cursor mut CURSOR,
    /// `(key, value)` where to start the walk.
    start: IterPairResult<T>,
    /// Phantom data for 'tx. As it is only used for `DbCursorRO`.
    _tx_phantom: PhantomData<&'tx T>,
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> ReverseWalker<'cursor, 'tx, T, CURSOR> {
    /// construct ReverseWalker
    pub fn new(cursor: &'cursor mut CURSOR, start: IterPairResult<T>) -> Self {
        Self { cursor, start, _tx_phantom: std::marker::PhantomData }
    }

    /// convert current [`ReverseWalker`] to [`Walker`] which iterate forwardly
    pub fn forward(self) -> Walker<'cursor, 'tx, T, CURSOR> {
        let start = self.cursor.current().transpose();
        Walker::new(self.cursor, start)
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for ReverseWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<(T::Key, T::Value), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }

        self.cursor.prev().transpose()
    }
}

/// Provides a range iterator to `Cursor` when handling `Table`.
/// Also check [`Walker`]
pub struct RangeWalker<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> {
    /// Cursor to be used to walk through the table.
    cursor: &'cursor mut CURSOR,
    /// `(key, value)` where to start the walk.
    start: IterPairResult<T>,
    /// exclusive `key` where to stop the walk.
    end_key: T::Key,
    /// flag whether is ended
    is_done: bool,
    /// Phantom data for 'tx. As it is only used for `DbCursorRO`.
    _tx_phantom: PhantomData<&'tx T>,
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for RangeWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<(T::Key, T::Value), Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_done {
            return None
        }

        let start = self.start.take();
        if start.is_some() {
            return start
        }

        let res = self.cursor.next().transpose()?;
        if let Ok((key, value)) = res {
            if key < self.end_key {
                Some(Ok((key, value)))
            } else {
                self.is_done = true;
                None
            }
        } else {
            Some(res)
        }
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> RangeWalker<'cursor, 'tx, T, CURSOR> {
    /// construct RangeWalker
    pub fn new(cursor: &'cursor mut CURSOR, start: IterPairResult<T>, end_key: T::Key) -> Self {
        Self { cursor, start, end_key, is_done: false, _tx_phantom: std::marker::PhantomData }
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
