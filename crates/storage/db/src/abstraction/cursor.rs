use std::{
    fmt,
    marker::PhantomData,
    ops::{Bound, RangeBounds},
};

use crate::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    table::{DupSort, Table, TableRow},
    DatabaseError,
};

/// A read-only cursor over table `T`.
pub trait DbCursorRO<'tx, T: Table> {
    /// Positions the cursor at the first entry in the table, returning it.
    fn first(&mut self) -> PairResult<T>;

    /// Seeks to the KV pair exactly at `key`.
    fn seek_exact(&mut self, key: T::Key) -> PairResult<T>;

    /// Seeks to the KV pair whose key is greater than or equal to `key`.
    fn seek(&mut self, key: T::Key) -> PairResult<T>;

    /// Position the cursor at the next KV pair, returning it.
    #[allow(clippy::should_implement_trait)]
    fn next(&mut self) -> PairResult<T>;

    /// Position the cursor at the previous KV pair, returning it.
    fn prev(&mut self) -> PairResult<T>;

    /// Positions the cursor at the last entry in the table, returning it.
    fn last(&mut self) -> PairResult<T>;

    /// Get the KV pair at the cursor's current position.
    fn current(&mut self) -> PairResult<T>;

    /// Get an iterator that walks through the table.
    ///
    /// If `start_key` is `None`, then the walker will start from the first entry of the table,
    /// otherwise it starts at the entry greater than or equal to the provided key.
    fn walk<'cursor>(
        &'cursor mut self,
        start_key: Option<T::Key>,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized;

    /// Get an iterator that walks over a range of keys in the table.
    fn walk_range<'cursor>(
        &'cursor mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized;

    /// Get an iterator that walks through the table in reverse order.
    ///
    /// If `start_key` is `None`, then the walker will start from the last entry of the table,
    /// otherwise it starts at the entry greater than or equal to the provided key.
    fn walk_back<'cursor>(
        &'cursor mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized;
}

/// A read-only cursor over the dup table `T`.
pub trait DbDupCursorRO<'tx, T: DupSort> {
    /// Positions the cursor at the next KV pair of the table, returning it.
    fn next_dup(&mut self) -> PairResult<T>;

    /// Positions the cursor at the next KV pair of the table, skipping duplicates.
    fn next_no_dup(&mut self) -> PairResult<T>;

    /// Positions the cursor at the next duplicate value of the current key.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T>;

    /// Positions the cursor at the entry greater than or equal to the provided key/subkey pair.
    ///
    /// # Note
    ///
    /// The position of the cursor might not correspond to the key/subkey pair if the entry does not
    /// exist.
    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T>;

    /// Get an iterator that walks through the dup table.
    ///
    /// The cursor will start at different points in the table depending on the values of `key` and
    /// `subkey`:
    ///
    /// | `key`  | `subkey` | **Equivalent starting position**        |
    /// |--------|----------|-----------------------------------------|
    /// | `None` | `None`   | [`DbCursorRO::first()`]                 |
    /// | `Some` | `None`   | [`DbCursorRO::seek()`]               |
    /// | `None` | `Some`   | [`DbDupCursorRO::seek_by_key_subkey()`] |
    /// | `Some` | `Some`   | [`DbDupCursorRO::seek_by_key_subkey()`] |
    fn walk_dup<'cursor>(
        &'cursor mut self,
        key: Option<T::Key>,
        subkey: Option<T::SubKey>,
    ) -> Result<DupWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized;
}

/// Read write cursor over table.
pub trait DbCursorRW<'tx, T: Table> {
    /// Database operation that will update an existing row if a specified value already
    /// exists in a table, and insert a new row if the specified value doesn't already exist
    fn upsert(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;

    /// Database operation that will insert a row at a given key. If the key is already
    /// present, the operation will result in an error.
    fn insert(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;

    /// Append value to next cursor item.
    ///
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use
    /// [`DbCursorRW::insert`].
    fn append(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;

    /// Delete current value that cursor points to
    fn delete_current(&mut self) -> Result<(), DatabaseError>;
}

/// Read Write Cursor over DupSorted table.
pub trait DbDupCursorRW<'tx, T: DupSort> {
    /// Delete all duplicate entries for current key.
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError>;

    /// Append duplicate value.
    ///
    /// This is efficient for pre-sorted data. If the data is not pre-sorted, use `insert`.
    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError>;
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

impl<'tx, T, CURSOR> fmt::Debug for Walker<'_, 'tx, T, CURSOR>
where
    T: Table,
    CURSOR: DbCursorRO<'tx, T> + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Walker").field("cursor", &self.cursor).field("start", &self.start).finish()
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for Walker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<TableRow<T>, DatabaseError>;
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

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRW<'tx, T> + DbCursorRO<'tx, T>>
    Walker<'cursor, 'tx, T, CURSOR>
{
    /// Delete current item that walker points to.
    pub fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.cursor.delete_current()
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

impl<'tx, T, CURSOR> fmt::Debug for ReverseWalker<'_, 'tx, T, CURSOR>
where
    T: Table,
    CURSOR: DbCursorRO<'tx, T> + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReverseWalker")
            .field("cursor", &self.cursor)
            .field("start", &self.start)
            .finish()
    }
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

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRW<'tx, T> + DbCursorRO<'tx, T>>
    ReverseWalker<'cursor, 'tx, T, CURSOR>
{
    /// Delete current item that walker points to.
    pub fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.cursor.delete_current()
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for ReverseWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<TableRow<T>, DatabaseError>;

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
    /// `key` where to stop the walk.
    end_key: Bound<T::Key>,
    /// flag whether is ended
    is_done: bool,
    /// Phantom data for 'tx. As it is only used for `DbCursorRO`.
    _tx_phantom: PhantomData<&'tx T>,
}

impl<'tx, T, CURSOR> fmt::Debug for RangeWalker<'_, 'tx, T, CURSOR>
where
    T: Table,
    CURSOR: DbCursorRO<'tx, T> + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RangeWalker")
            .field("cursor", &self.cursor)
            .field("start", &self.start)
            .field("end_key", &self.end_key)
            .field("is_done", &self.is_done)
            .finish()
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> std::iter::Iterator
    for RangeWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<TableRow<T>, DatabaseError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_done {
            return None
        }

        let next_item = self.start.take().or_else(|| self.cursor.next().transpose());

        match next_item {
            Some(Ok((key, value))) => match &self.end_key {
                Bound::Included(end_key) if &key <= end_key => Some(Ok((key, value))),
                Bound::Excluded(end_key) if &key < end_key => Some(Ok((key, value))),
                Bound::Unbounded => Some(Ok((key, value))),
                _ => {
                    self.is_done = true;
                    None
                }
            },
            Some(res @ Err(_)) => Some(res),
            None if matches!(self.end_key, Bound::Unbounded) => {
                self.is_done = true;
                None
            }
            _ => None,
        }
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRO<'tx, T>> RangeWalker<'cursor, 'tx, T, CURSOR> {
    /// construct RangeWalker
    pub fn new(
        cursor: &'cursor mut CURSOR,
        start: IterPairResult<T>,
        end_key: Bound<T::Key>,
    ) -> Self {
        // mark done if range is empty.
        let is_done = match start {
            Some(Ok((ref start_key, _))) => match &end_key {
                Bound::Included(end_key) if start_key > end_key => true,
                Bound::Excluded(end_key) if start_key >= end_key => true,
                _ => false,
            },
            None => true,
            _ => false,
        };
        Self { cursor, start, end_key, is_done, _tx_phantom: std::marker::PhantomData }
    }
}

impl<'cursor, 'tx, T: Table, CURSOR: DbCursorRW<'tx, T> + DbCursorRO<'tx, T>>
    RangeWalker<'cursor, 'tx, T, CURSOR>
{
    /// Delete current item that walker points to.
    pub fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.cursor.delete_current()
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

impl<'tx, T, CURSOR> fmt::Debug for DupWalker<'_, 'tx, T, CURSOR>
where
    T: DupSort,
    CURSOR: DbDupCursorRO<'tx, T> + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DupWalker")
            .field("cursor", &self.cursor)
            .field("start", &self.start)
            .finish()
    }
}

impl<'cursor, 'tx, T: DupSort, CURSOR: DbCursorRW<'tx, T> + DbDupCursorRO<'tx, T>>
    DupWalker<'cursor, 'tx, T, CURSOR>
{
    /// Delete current item that walker points to.
    pub fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.cursor.delete_current()
    }
}

impl<'cursor, 'tx, T: DupSort, CURSOR: DbDupCursorRO<'tx, T>> std::iter::Iterator
    for DupWalker<'cursor, 'tx, T, CURSOR>
{
    type Item = Result<TableRow<T>, DatabaseError>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup().transpose()
    }
}
