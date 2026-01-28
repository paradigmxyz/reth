use crate::{
    codec_try_optional,
    error::{mdbx_result, MdbxResult},
    flags::*,
    tx::{
        assertions,
        iter::{Iter, IterDup, IterDupVals, IterKeyVals},
        TxPtrAccess,
    },
    Database, MdbxError, ReadResult, TableObject, TransactionKind, RW,
};
use ffi::{
    MDBX_cursor_op, MDBX_FIRST, MDBX_FIRST_DUP, MDBX_GET_BOTH, MDBX_GET_BOTH_RANGE,
    MDBX_GET_CURRENT, MDBX_GET_MULTIPLE, MDBX_LAST, MDBX_LAST_DUP, MDBX_NEXT, MDBX_NEXT_DUP,
    MDBX_NEXT_MULTIPLE, MDBX_NEXT_NODUP, MDBX_PREV, MDBX_PREV_DUP, MDBX_PREV_MULTIPLE,
    MDBX_PREV_NODUP, MDBX_SET, MDBX_SET_KEY, MDBX_SET_LOWERBOUND, MDBX_SET_RANGE,
};
use std::{ffi::c_void, fmt, marker::PhantomData, ptr};

/// A cursor for navigating the items within a database.
///
/// The cursor is generic over the transaction kind `K` and the access type `A`.
/// The access type determines how the cursor accesses the underlying transaction
/// pointer, allowing the same cursor implementation to work with different
/// transaction implementations.
pub struct Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    access: &'tx A,
    cursor: *mut ffi::MDBX_cursor,
    db: Database,
    _kind: PhantomData<K>,
}

impl<'tx, K, A> Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    /// Creates a new cursor from a reference to a transaction access type.
    pub(crate) fn new(access: &'tx A, db: Database) -> MdbxResult<Self> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        access.with_txn_ptr(|txn_ptr| unsafe {
            mdbx_result(ffi::mdbx_cursor_open(txn_ptr, db.dbi(), &mut cursor))
        })??;
        Ok(Self { access, cursor, db, _kind: PhantomData })
    }

    /// Creates a cursor from a raw MDBX cursor pointer.
    ///
    /// This function must only be used when you are certain that the provided
    /// cursor pointer is valid and associated with the given access type.
    pub(crate) const fn new_raw(access: &'tx A, cursor: *mut ffi::MDBX_cursor, db: Database) -> Self
    where
        A: Sized,
    {
        Self { access, cursor, db, _kind: PhantomData }
    }

    /// Helper function for `Clone`. This should only be invoked within
    /// a `with_txn_ptr` call to ensure safety.
    fn new_at_position(other: &Self) -> MdbxResult<Self> {
        unsafe {
            let cursor = ffi::mdbx_cursor_create(ptr::null_mut());

            let res = ffi::mdbx_cursor_copy(other.cursor(), cursor);

            let s = Self { access: other.access, cursor, db: other.db, _kind: PhantomData };

            mdbx_result(res)?;

            Ok(s)
        }
    }

    /// Returns a reference to the transaction access type.
    pub(crate) const fn access(&self) -> &'tx A
    where
        A: Sized,
    {
        self.access
    }

    /// Returns a raw pointer to the underlying MDBX cursor.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the cursor.
    pub const fn cursor(&self) -> *mut ffi::MDBX_cursor {
        self.cursor
    }

    /// Returns the database associated with this cursor.
    pub const fn db(&self) -> Database {
        self.db
    }

    /// Returns the flags of the database associated with this cursor.
    pub const fn db_flags(&self) -> DatabaseFlags {
        self.db.flags()
    }

    /// Returns `true` if the cursor is at EOF or not positioned.
    ///
    /// This can be used to check if the cursor has valid data before
    /// performing operations that depend on cursor position.
    pub fn is_eof(&self) -> bool {
        self.access
            .with_txn_ptr(|_| unsafe { ffi::mdbx_cursor_eof(self.cursor) })
            .unwrap_or(ffi::MDBX_RESULT_TRUE) ==
            ffi::MDBX_RESULT_TRUE
    }

    /// Validates that the database has the DUP_SORT flag set.
    #[inline(always)]
    fn require_dup_sort(&self) -> MdbxResult<()> {
        self.db
            .flags()
            .contains(DatabaseFlags::DUP_SORT)
            .then_some(())
            .ok_or(MdbxError::RequiresDupSort)
    }

    /// Validates that the database has the DUP_FIXED flag set.
    #[inline(always)]
    fn require_dup_fixed(&self) -> MdbxResult<()> {
        self.db
            .flags()
            .contains(DatabaseFlags::DUP_FIXED)
            .then_some(())
            .ok_or(MdbxError::RequiresDupFixed)
    }

    /// Retrieves a key/data pair from the cursor. Depending on the cursor op,
    /// the current key may be returned.
    fn get<Key, Value>(
        &self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> ReadResult<(Option<Key>, Value, bool)>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let mut key_val = slice_to_val(key);
        let mut data_val = slice_to_val(data);
        let key_ptr = key_val.iov_base;
        let data_ptr = data_val.iov_base;

        self.access.with_txn_ptr(|txn| {
            // SAFETY:
            // The cursor is valid as long as self is alive.
            // The transaction is also valid as long as self is alive.
            // The data in key_val and data_val is valid as long as the
            // transaction is alive, provided the page is not dirty.
            // decode_val checks for dirty pages and copies data if needed.
            unsafe {
                let v = mdbx_result(ffi::mdbx_cursor_get(
                    self.cursor,
                    &mut key_val,
                    &mut data_val,
                    op,
                ))?;
                assert_ne!(data_ptr, data_val.iov_base);
                let key_out = {
                    // MDBX wrote in new key
                    if ptr::eq(key_ptr, key_val.iov_base) {
                        None
                    } else {
                        Some(Key::decode_val::<K>(txn, key_val)?)
                    }
                };
                let data_out = Value::decode_val::<K>(txn, data_val)?;
                Ok((key_out, data_out, v))
            }
        })?
    }

    fn get_value<Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        let (_, v, result_true) = codec_try_optional!(self.get::<(), Value>(key, data, op));
        if result_true {
            return Ok(None);
        }
        Ok(Some(v))
    }

    fn get_full<Key, Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let (k, v, result_true) = codec_try_optional!(self.get(key, data, op));
        if result_true {
            return Ok(None);
        }
        Ok(Some((k.unwrap(), v)))
    }

    /// Position at first key/data item.
    pub fn first<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_FIRST)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at first data item of current key.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn first_dup<Value>(&mut self) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_value(None, None, MDBX_FIRST_DUP)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at key/data pair.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn get_both<Value>(&mut self, k: &[u8], v: &[u8]) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_value(Some(k), Some(v), MDBX_GET_BOTH)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at given key and at first data greater than or
    /// equal to specified data.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn get_both_range<Value>(&mut self, k: &[u8], v: &[u8]) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_value(Some(k), Some(v), MDBX_GET_BOTH_RANGE)
    }

    /// Return key/data at current cursor position.
    pub fn get_current<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_GET_CURRENT)
    }

    /// [`DatabaseFlags::DUP_FIXED`]-only: Return up to a page of duplicate data items from current
    /// cursor position. Move cursor to prepare for [`Self::next_multiple()`].
    ///
    /// Returns [`MdbxError::RequiresDupFixed`] if the database does not have the
    /// [`DatabaseFlags::DUP_FIXED`] flag set.
    pub fn get_multiple<Value>(&mut self) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        self.require_dup_fixed()?;
        self.get_value(None, None, MDBX_GET_MULTIPLE)
    }

    /// Position at last key/data item.
    pub fn last<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_LAST)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at last data item of current key.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn last_dup<Value>(&mut self) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_value(None, None, MDBX_LAST_DUP)
    }

    /// Position at next data item
    #[expect(clippy::should_implement_trait)]
    pub fn next<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_NEXT)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at next data item of current key.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn next_dup<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_full(None, None, MDBX_NEXT_DUP)
    }

    /// [`DatabaseFlags::DUP_FIXED`]-only: Return up to a page of duplicate data items from next
    /// cursor position. Move cursor to prepare for `MDBX_NEXT_MULTIPLE`.
    ///
    /// Returns [`MdbxError::RequiresDupFixed`] if the database does not have the
    /// [`DatabaseFlags::DUP_FIXED`] flag set.
    pub fn next_multiple<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.require_dup_fixed()?;
        self.get_full(None, None, MDBX_NEXT_MULTIPLE)
    }

    /// Position at first data item of next key.
    pub fn next_nodup<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_NEXT_NODUP)
    }

    /// Position at previous data item.
    pub fn prev<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_PREV)
    }

    /// [`DatabaseFlags::DUP_SORT`]-only: Position at previous data item of current key.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    pub fn prev_dup<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.require_dup_sort()?;
        self.get_full(None, None, MDBX_PREV_DUP)
    }

    /// Position at last data item of previous key.
    pub fn prev_nodup<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.get_full(None, None, MDBX_PREV_NODUP)
    }

    /// Position at specified key.
    pub fn set<Value>(&mut self, key: &[u8]) -> ReadResult<Option<Value>>
    where
        Value: TableObject<'tx>,
    {
        assertions::debug_assert_integer_key(self.db.flags(), key);
        self.get_value(Some(key), None, MDBX_SET)
    }

    /// Position at specified key, return both key and data.
    pub fn set_key<Key, Value>(&mut self, key: &[u8]) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        assertions::debug_assert_integer_key(self.db.flags(), key);
        self.get_full(Some(key), None, MDBX_SET_KEY)
    }

    /// Position at first key greater than or equal to specified key.
    pub fn set_range<Key, Value>(&mut self, key: &[u8]) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        assertions::debug_assert_integer_key(self.db.flags(), key);
        self.get_full(Some(key), None, MDBX_SET_RANGE)
    }

    /// [`DatabaseFlags::DUP_FIXED`]-only: Position at previous page and return up to a page of
    /// duplicate data items.
    ///
    /// Returns [`MdbxError::RequiresDupFixed`] if the database does not have the
    /// [`DatabaseFlags::DUP_FIXED`] flag set.
    pub fn prev_multiple<Key, Value>(&mut self) -> ReadResult<Option<(Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        self.require_dup_fixed()?;
        self.get_full(None, None, MDBX_PREV_MULTIPLE)
    }

    /// Position at first key-value pair greater than or equal to specified, return both key and
    /// data, and the return code depends on an exact match.
    ///
    /// For non DupSort-ed collections this works the same as [`Self::set_range()`], but returns
    /// [false] if key found exactly and [true] if greater key was found.
    ///
    /// For DupSort-ed a data value is taken into account for duplicates, i.e. for a pairs/tuples of
    /// a key and an each data value of duplicates. Returns [false] if key-value pair found
    /// exactly and [true] if the next pair was returned.
    pub fn set_lowerbound<Key, Value>(
        &mut self,
        key: &[u8],
    ) -> ReadResult<Option<(bool, Key, Value)>>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        assertions::debug_assert_integer_key(self.db.flags(), key);
        let (k, v, found) = codec_try_optional!(self.get(Some(key), None, MDBX_SET_LOWERBOUND));

        Ok(Some((found, k.unwrap(), v)))
    }

    /// Returns an iterator over database items.
    ///
    /// The iterator will begin with item next after the cursor, and continue
    /// until the end of the database. For new cursors, the iterator will begin
    /// with the first item in the database.
    ///
    /// If the cursor is at EOF or not positioned (e.g., after exhausting a
    /// previous iteration), it will be repositioned to the first item.
    ///
    /// For databases with duplicate data items ([`DatabaseFlags::DUP_SORT`]),
    /// the duplicate data items of each key will be returned before moving on
    /// to the next key.
    pub fn iter<'cur, Key, Value>(&'cur mut self) -> IterKeyVals<'tx, 'cur, K, A, Key, Value>
    where
        'tx: 'cur,
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        if self.is_eof() {
            // Reposition to first item
            match self.first::<Key, Value>() {
                Ok(Some(first)) => return IterKeyVals::from_ref_with(self, first),
                Ok(None) | Err(_) => return IterKeyVals::end_from_ref(self),
            }
        }
        IterKeyVals::from_ref(self)
    }

    /// Returns an iterator over database items as slices.
    ///
    /// The iterator will begin with item next after the cursor, and continue
    /// until the end of the database. For new cursors, the iterator will begin
    /// with the first item in the database.
    pub fn iter_slices<'cur>(&'cur mut self) -> IterKeyVals<'tx, 'cur, K, A>
    where
        'tx: 'cur,
    {
        IterKeyVals::from_ref(self)
    }

    /// Iterate over database items starting from the beginning of the database.
    ///
    /// For databases with duplicate data items ([`DatabaseFlags::DUP_SORT`]),
    /// the duplicate data items of each key will be returned before moving on
    /// to the next key.
    pub fn iter_start<'cur, Key, Value>(
        &'cur mut self,
    ) -> ReadResult<Iter<'tx, 'cur, K, A, Key, Value>>
    where
        'tx: 'cur,
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let Some(first) = self.first()? else {
            return Ok(Iter::end_from_ref(self));
        };

        Ok(Iter::from_ref_with(self, first))
    }

    /// Iterate over database items starting from the given key.
    ///
    /// For databases with duplicate data items ([`DatabaseFlags::DUP_SORT`]),
    /// the duplicate data items of each key will be returned before moving on
    /// to the next key.
    pub fn iter_from<'cur, Key, Value>(
        &'cur mut self,
        key: &[u8],
    ) -> ReadResult<Iter<'tx, 'cur, K, A, Key, Value>>
    where
        'tx: 'cur,
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let Some(first) = self.set_range::<Key, Value>(key)? else {
            return Ok(Iter::end_from_ref(self));
        };

        Ok(Iter::from_ref_with(self, first))
    }

    /// Iterate over duplicate database items.
    ///
    /// The iterator will produce an iterator for each key in the database,
    /// yielding all duplicate data items for that key.
    ///
    /// Like [`Self::iter`], this function will start with the key AFTER the
    /// current cursor position, and continue until the end of the database.
    /// For new cursors, the iterator will begin with the first key in the
    /// database.
    ///
    /// If the cursor is at EOF or not positioned (e.g., after exhausting a
    /// previous iteration), it will be repositioned to the first item.
    pub fn iter_dup<'cur, Key, Value>(&'cur mut self) -> IterDup<'tx, 'cur, K, A, Key, Value>
    where
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        if self.is_eof() {
            match self.first::<Key, Value>() {
                Ok(Some(first)) => return IterDup::from_ref_with(self, first),
                Ok(None) | Err(_) => return IterDup::end_from_ref(self),
            }
        }
        IterDup::from_ref(self)
    }

    /// Iterate over duplicate database items starting from the beginning of the
    /// database. Each item will be returned as an iterator of its duplicates.
    pub fn iter_dup_start<'cur, Key, Value>(
        &'cur mut self,
    ) -> ReadResult<IterDup<'tx, 'cur, K, A, Key, Value>>
    where
        'tx: 'cur,
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let Some(first) = self.first()? else {
            return Ok(IterDup::end_from_ref(self));
        };

        Ok(IterDup::from_ref_with(self, first))
    }

    /// Iterate over duplicate items in the database starting from the given
    /// key. Each item will be returned as an iterator of its duplicates.
    pub fn iter_dup_from<'cur, Key, Value>(
        &'cur mut self,
        key: &[u8],
    ) -> ReadResult<IterDup<'tx, 'cur, K, A, Key, Value>>
    where
        'tx: 'cur,
        Key: TableObject<'tx>,
        Value: TableObject<'tx>,
    {
        let Some(first) = self.set_range(key)? else {
            return Ok(IterDup::end_from_ref(self));
        };

        Ok(IterDup::from_ref_with(self, first))
    }

    /// Iterate over the duplicates of the item in the database with the given
    /// key.
    pub fn iter_dup_of<'cur, Key, Value>(
        &'cur mut self,
        key: &[u8],
    ) -> ReadResult<IterDupVals<'tx, 'cur, K, A, Key, Value>>
    where
        'tx: 'cur,
        Key: TableObject<'tx> + PartialEq,
        Value: TableObject<'tx>,
    {
        let Some(first) = self.set_key(key.as_ref())? else {
            return Ok(IterDupVals::end_from_ref(self));
        };

        Ok(IterDupVals::from_ref_with(self, first))
    }
}

impl<'tx, A> Cursor<'tx, RW, A>
where
    A: TxPtrAccess,
{
    /// Puts a key/data pair into the database. The cursor will be positioned at
    /// the new data item, or on failure usually near it.
    pub fn put(&mut self, key: &[u8], data: &[u8], flags: WriteFlags) -> MdbxResult<()> {
        #[cfg(debug_assertions)]
        self.access.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid, getting env and stat for assertion only
            let env_ptr = unsafe { ffi::mdbx_txn_env(txn_ptr) };
            let mut stat: ffi::MDBX_stat = unsafe { std::mem::zeroed() };
            unsafe {
                ffi::mdbx_env_stat_ex(
                    env_ptr,
                    std::ptr::null(),
                    &mut stat,
                    std::mem::size_of::<ffi::MDBX_stat>(),
                )
            };
            let pagesize = stat.ms_psize as usize;
            assertions::debug_assert_put(pagesize, self.db.flags(), key, data);
        })?;

        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };
        mdbx_result(self.access.with_txn_ptr(|_| unsafe {
            ffi::mdbx_cursor_put(self.cursor, &key_val, &mut data_val, flags.bits())
        })?)?;

        Ok(())
    }

    /// Deletes the current key/data pair.
    ///
    /// ### Flags
    ///
    /// [`WriteFlags::NO_DUP_DATA`] may be used to delete all data items for the
    /// current key, if the database was opened with [`DatabaseFlags::DUP_SORT`].
    pub fn del(&mut self, flags: WriteFlags) -> MdbxResult<()> {
        mdbx_result(
            self.access
                .with_txn_ptr(|_| unsafe { ffi::mdbx_cursor_del(self.cursor, flags.bits()) })?,
        )?;

        Ok(())
    }

    /// Appends a key/data pair to the end of the database.
    ///
    /// The key must be greater than all existing keys (or less than, for
    /// [`DatabaseFlags::REVERSE_KEY`] tables). This is more efficient than
    /// [`Cursor::put`] when adding data in sorted order.
    ///
    /// In debug builds, this method asserts that the key ordering constraint is
    /// satisfied.
    pub fn append(&mut self, key: &[u8], data: &[u8]) -> MdbxResult<()> {
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };

        mdbx_result(self.access.with_txn_ptr(|_txn_ptr| {
            #[cfg(debug_assertions)]
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe {
                crate::tx::ops::debug_assert_append(
                    _txn_ptr,
                    self.db.dbi(),
                    self.db.flags(),
                    key,
                    data,
                )
            };

            // SAFETY: cursor and txn_ptr are valid.
            unsafe {
                ffi::mdbx_cursor_put(
                    self.cursor,
                    &key_val,
                    &mut data_val,
                    WriteFlags::APPEND.bits(),
                )
            }
        })?)?;

        Ok(())
    }

    /// Appends duplicate data for [`DatabaseFlags::DUP_SORT`] databases.
    ///
    /// The data must be greater than all existing data for this key (or less
    /// than, for [`DatabaseFlags::REVERSE_DUP`] tables). This is more efficient
    /// than [`Cursor::put`] when adding duplicates in sorted order.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    ///
    /// In debug builds, this method asserts that the data ordering constraint
    /// is satisfied.
    pub fn append_dup(&mut self, key: &[u8], data: &[u8]) -> MdbxResult<()> {
        self.require_dup_sort()?;

        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };

        mdbx_result(self.access.with_txn_ptr(|_txn_ptr| {
            #[cfg(debug_assertions)]
            // SAFETY: _txn_ptr is valid from with_txn_ptr.
            unsafe {
                crate::tx::ops::debug_assert_append_dup(
                    _txn_ptr,
                    self.db.dbi(),
                    self.db.flags(),
                    key,
                    data,
                )
            };

            // SAFETY: cursor and txn_ptr are valid.
            unsafe {
                ffi::mdbx_cursor_put(
                    self.cursor,
                    &key_val,
                    &mut data_val,
                    WriteFlags::APPEND_DUP.bits(),
                )
            }
        })?)?;

        Ok(())
    }
}

impl<'tx, K, A> Clone for Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    fn clone(&self) -> Self {
        self.access.with_txn_ptr(|_| Self::new_at_position(self).unwrap()).unwrap()
    }
}

impl<'tx, K, A> fmt::Debug for Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cursor").finish_non_exhaustive()
    }
}

impl<'tx, K, A> Drop for Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    fn drop(&mut self) {
        // MDBX cursors MUST be closed. Failure to do so is a memory leak.
        //
        // To be able to close a cursor of a timed out transaction, we need to
        // renew it first. Hence the usage of `with_txn_ptr_for_cleanup` here.
        let _ = self
            .access
            .with_txn_ptr_for_cleanup(|_| unsafe { ffi::mdbx_cursor_close(self.cursor) });
    }
}

const fn slice_to_val(slice: Option<&[u8]>) -> ffi::MDBX_val {
    match slice {
        Some(slice) => {
            ffi::MDBX_val { iov_len: slice.len(), iov_base: slice.as_ptr() as *mut c_void }
        }
        None => ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() },
    }
}

unsafe impl<'tx, K, A> Send for Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess + Sync,
{
}
unsafe impl<'tx, K, A> Sync for Cursor<'tx, K, A>
where
    K: TransactionKind,
    A: TxPtrAccess + Sync,
{
}

/// A read-only cursor for a synchronized transaction.
pub type RoCursorSync<'tx> = Cursor<'tx, crate::RO, crate::tx::PtrSyncInner<crate::RO>>;

/// A read-write cursor for a synchronized transaction.
pub type RwCursorSync<'tx> = Cursor<'tx, crate::RW, crate::tx::PtrSyncInner<crate::RW>>;

/// A read-only cursor for an unsynchronized transaction.
pub type RoCursorUnsync<'tx> = Cursor<'tx, crate::RO, crate::tx::RoGuard>;

/// A read-write cursor for an unsynchronized transaction.
pub type RwCursorUnsync<'tx> = Cursor<'tx, crate::RW, crate::tx::RwUnsync>;
