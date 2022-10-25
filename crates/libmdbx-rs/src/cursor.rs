use crate::{
    database::Database,
    error::{mdbx_result, Error, Result},
    flags::*,
    mdbx_try_optional,
    transaction::{txn_execute, TransactionKind, RW},
    EnvironmentKind, TableObject, Transaction,
};
use ffi::{
    MDBX_cursor_op, MDBX_FIRST, MDBX_FIRST_DUP, MDBX_GET_BOTH, MDBX_GET_BOTH_RANGE,
    MDBX_GET_CURRENT, MDBX_GET_MULTIPLE, MDBX_LAST, MDBX_LAST_DUP, MDBX_NEXT, MDBX_NEXT_DUP,
    MDBX_NEXT_MULTIPLE, MDBX_NEXT_NODUP, MDBX_PREV, MDBX_PREV_DUP, MDBX_PREV_MULTIPLE,
    MDBX_PREV_NODUP, MDBX_SET, MDBX_SET_KEY, MDBX_SET_LOWERBOUND, MDBX_SET_RANGE,
};
use libc::{c_uint, c_void};
use parking_lot::Mutex;
use std::{borrow::Cow, fmt, marker::PhantomData, mem, ptr, result, sync::Arc};

/// A cursor for navigating the items within a database.
pub struct Cursor<'txn, K>
where
    K: TransactionKind,
{
    txn: Arc<Mutex<*mut ffi::MDBX_txn>>,
    cursor: *mut ffi::MDBX_cursor,
    _marker: PhantomData<fn(&'txn (), K)>,
}

impl<'txn, K> Cursor<'txn, K>
where
    K: TransactionKind,
{
    pub(crate) fn new<E: EnvironmentKind>(
        txn: &'txn Transaction<K, E>,
        db: &Database<'txn>,
    ) -> Result<Self> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();

        let txn = txn.txn_mutex();
        unsafe {
            mdbx_result(txn_execute(&*txn, |txn| {
                ffi::mdbx_cursor_open(txn, db.dbi(), &mut cursor)
            }))?;
        }
        Ok(Self {
            txn,
            cursor,
            _marker: PhantomData,
        })
    }

    fn new_at_position(other: &Self) -> Result<Self> {
        unsafe {
            let cursor = ffi::mdbx_cursor_create(ptr::null_mut());

            let res = ffi::mdbx_cursor_copy(other.cursor(), cursor);

            let s = Self {
                txn: other.txn.clone(),
                cursor,
                _marker: PhantomData,
            };

            mdbx_result(res)?;

            Ok(s)
        }
    }

    /// Returns a raw pointer to the underlying MDBX cursor.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the cursor.
    pub fn cursor(&self) -> *mut ffi::MDBX_cursor {
        self.cursor
    }

    /// Retrieves a key/data pair from the cursor. Depending on the cursor op,
    /// the current key may be returned.
    fn get<Key, Value>(
        &self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> Result<(Option<Key>, Value, bool)>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        unsafe {
            let mut key_val = slice_to_val(key);
            let mut data_val = slice_to_val(data);
            let key_ptr = key_val.iov_base;
            let data_ptr = data_val.iov_base;
            txn_execute(&*self.txn, |txn| {
                let v = mdbx_result(ffi::mdbx_cursor_get(
                    self.cursor,
                    &mut key_val,
                    &mut data_val,
                    op,
                ))?;
                assert_ne!(data_ptr, data_val.iov_base);
                let key_out = {
                    // MDBX wrote in new key
                    if key_ptr != key_val.iov_base {
                        Some(Key::decode_val::<K>(txn, &key_val)?)
                    } else {
                        None
                    }
                };
                let data_out = Value::decode_val::<K>(txn, &data_val)?;
                Ok((key_out, data_out, v))
            })
        }
    }

    fn get_value<Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        let (_, v, _) = mdbx_try_optional!(self.get::<(), Value>(key, data, op));

        Ok(Some(v))
    }

    fn get_full<Key, Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        let (k, v, _) = mdbx_try_optional!(self.get(key, data, op));

        Ok(Some((k.unwrap(), v)))
    }

    /// Position at first key/data item.
    pub fn first<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_FIRST)
    }

    /// [DatabaseFlags::DUP_SORT]-only: Position at first data item of current key.
    pub fn first_dup<Value>(&mut self) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(None, None, MDBX_FIRST_DUP)
    }

    /// [DatabaseFlags::DUP_SORT]-only: Position at key/data pair.
    pub fn get_both<Value>(&mut self, k: &[u8], v: &[u8]) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(Some(k), Some(v), MDBX_GET_BOTH)
    }

    /// [DatabaseFlags::DUP_SORT]-only: Position at given key and at first data greater than or equal to specified data.
    pub fn get_both_range<Value>(&mut self, k: &[u8], v: &[u8]) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(Some(k), Some(v), MDBX_GET_BOTH_RANGE)
    }

    /// Return key/data at current cursor position.
    pub fn get_current<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_GET_CURRENT)
    }

    /// DupFixed-only: Return up to a page of duplicate data items from current cursor position.
    /// Move cursor to prepare for [Self::next_multiple()].
    pub fn get_multiple<Value>(&mut self) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(None, None, MDBX_GET_MULTIPLE)
    }

    /// Position at last key/data item.
    pub fn last<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_LAST)
    }

    /// DupSort-only: Position at last data item of current key.
    pub fn last_dup<Value>(&mut self) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(None, None, MDBX_LAST_DUP)
    }

    /// Position at next data item
    #[allow(clippy::should_implement_trait)]
    pub fn next<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_NEXT)
    }

    /// [DatabaseFlags::DUP_SORT]-only: Position at next data item of current key.
    pub fn next_dup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_NEXT_DUP)
    }

    /// [DatabaseFlags::DUP_FIXED]-only: Return up to a page of duplicate data items from next cursor position. Move cursor to prepare for MDBX_NEXT_MULTIPLE.
    pub fn next_multiple<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_NEXT_MULTIPLE)
    }

    /// Position at first data item of next key.
    pub fn next_nodup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_NEXT_NODUP)
    }

    /// Position at previous data item.
    pub fn prev<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_PREV)
    }

    /// [DatabaseFlags::DUP_SORT]-only: Position at previous data item of current key.
    pub fn prev_dup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_PREV_DUP)
    }

    /// Position at last data item of previous key.
    pub fn prev_nodup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_PREV_NODUP)
    }

    /// Position at specified key.
    pub fn set<Value>(&mut self, key: &[u8]) -> Result<Option<Value>>
    where
        Value: TableObject<'txn>,
    {
        self.get_value(Some(key), None, MDBX_SET)
    }

    /// Position at specified key, return both key and data.
    pub fn set_key<Key, Value>(&mut self, key: &[u8]) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(Some(key), None, MDBX_SET_KEY)
    }

    /// Position at first key greater than or equal to specified key.
    pub fn set_range<Key, Value>(&mut self, key: &[u8]) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(Some(key), None, MDBX_SET_RANGE)
    }

    /// [DatabaseFlags::DUP_FIXED]-only: Position at previous page and return up to a page of duplicate data items.
    pub fn prev_multiple<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        self.get_full(None, None, MDBX_PREV_MULTIPLE)
    }

    /// Position at first key-value pair greater than or equal to specified, return both key and data, and the return code depends on a exact match.
    ///
    /// For non DupSort-ed collections this works the same as [Self::set_range()], but returns [false] if key found exactly and [true] if greater key was found.
    ///
    /// For DupSort-ed a data value is taken into account for duplicates, i.e. for a pairs/tuples of a key and an each data value of duplicates.
    /// Returns [false] if key-value pair found exactly and [true] if the next pair was returned.
    pub fn set_lowerbound<Key, Value>(&mut self, key: &[u8]) -> Result<Option<(bool, Key, Value)>>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        let (k, v, found) = mdbx_try_optional!(self.get(Some(key), None, MDBX_SET_LOWERBOUND));

        Ok(Some((found, k.unwrap(), v)))
    }

    /// Iterate over database items. The iterator will begin with item next
    /// after the cursor, and continue until the end of the database. For new
    /// cursors, the iterator will begin with the first item in the database.
    ///
    /// For databases with duplicate data items ([DatabaseFlags::DUP_SORT]), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    pub fn iter<Key, Value>(&mut self) -> Iter<'txn, '_, K, Key, Value>
    where
        Self: Sized,
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        Iter::new(self, ffi::MDBX_NEXT, ffi::MDBX_NEXT)
    }

    /// Iterate over database items starting from the beginning of the database.
    ///
    /// For databases with duplicate data items ([DatabaseFlags::DUP_SORT]), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    pub fn iter_start<Key, Value>(&mut self) -> Iter<'txn, '_, K, Key, Value>
    where
        Self: Sized,
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        Iter::new(self, ffi::MDBX_FIRST, ffi::MDBX_NEXT)
    }

    /// Iterate over database items starting from the given key.
    ///
    /// For databases with duplicate data items ([DatabaseFlags::DUP_SORT]), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    pub fn iter_from<Key, Value>(&mut self, key: &[u8]) -> Iter<'txn, '_, K, Key, Value>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        let res: Result<Option<((), ())>> = self.set_range(key);
        if let Err(error) = res {
            return Iter::Err(Some(error));
        };
        Iter::new(self, ffi::MDBX_GET_CURRENT, ffi::MDBX_NEXT)
    }

    /// Iterate over duplicate database items. The iterator will begin with the
    /// item next after the cursor, and continue until the end of the database.
    /// Each item will be returned as an iterator of its duplicates.
    pub fn iter_dup<Key, Value>(&mut self) -> IterDup<'txn, '_, K, Key, Value>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        IterDup::new(self, ffi::MDBX_NEXT as u32)
    }

    /// Iterate over duplicate database items starting from the beginning of the
    /// database. Each item will be returned as an iterator of its duplicates.
    pub fn iter_dup_start<Key, Value>(&mut self) -> IterDup<'txn, '_, K, Key, Value>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        IterDup::new(self, ffi::MDBX_FIRST as u32)
    }

    /// Iterate over duplicate items in the database starting from the given
    /// key. Each item will be returned as an iterator of its duplicates.
    pub fn iter_dup_from<Key, Value>(&mut self, key: &[u8]) -> IterDup<'txn, '_, K, Key, Value>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        let res: Result<Option<((), ())>> = self.set_range(key);
        if let Err(error) = res {
            return IterDup::Err(Some(error));
        };
        IterDup::new(self, ffi::MDBX_GET_CURRENT as u32)
    }

    /// Iterate over the duplicates of the item in the database with the given key.
    pub fn iter_dup_of<Key, Value>(&mut self, key: &[u8]) -> Iter<'txn, '_, K, Key, Value>
    where
        Key: TableObject<'txn>,
        Value: TableObject<'txn>,
    {
        let res: Result<Option<()>> = self.set(key);
        match res {
            Ok(Some(_)) => (),
            Ok(None) => {
                let _: Result<Option<((), ())>> = self.last();
                return Iter::new(self, ffi::MDBX_NEXT, ffi::MDBX_NEXT);
            }
            Err(error) => return Iter::Err(Some(error)),
        };
        Iter::new(self, ffi::MDBX_GET_CURRENT, ffi::MDBX_NEXT_DUP)
    }
}

impl<'txn> Cursor<'txn, RW> {
    /// Puts a key/data pair into the database. The cursor will be positioned at
    /// the new data item, or on failure usually near it.
    pub fn put(&mut self, key: &[u8], data: &[u8], flags: WriteFlags) -> Result<()> {
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        };
        mdbx_result(unsafe {
            txn_execute(&*self.txn, |_| {
                ffi::mdbx_cursor_put(self.cursor, &key_val, &mut data_val, flags.bits())
            })
        })?;

        Ok(())
    }

    /// Deletes the current key/data pair.
    ///
    /// ### Flags
    ///
    /// [WriteFlags::NO_DUP_DATA] may be used to delete all data items for the
    /// current key, if the database was opened with [DatabaseFlags::DUP_SORT].
    pub fn del(&mut self, flags: WriteFlags) -> Result<()> {
        mdbx_result(unsafe {
            txn_execute(&*self.txn, |_| {
                ffi::mdbx_cursor_del(self.cursor, flags.bits())
            })
        })?;

        Ok(())
    }
}

impl<'txn, K> Clone for Cursor<'txn, K>
where
    K: TransactionKind,
{
    fn clone(&self) -> Self {
        txn_execute(&*self.txn, |_| Self::new_at_position(self).unwrap())
    }
}

impl<'txn, K> fmt::Debug for Cursor<'txn, K>
where
    K: TransactionKind,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("Cursor").finish()
    }
}

impl<'txn, K> Drop for Cursor<'txn, K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        txn_execute(&*self.txn, |_| unsafe {
            ffi::mdbx_cursor_close(self.cursor)
        })
    }
}

unsafe fn slice_to_val(slice: Option<&[u8]>) -> ffi::MDBX_val {
    match slice {
        Some(slice) => ffi::MDBX_val {
            iov_len: slice.len(),
            iov_base: slice.as_ptr() as *mut c_void,
        },
        None => ffi::MDBX_val {
            iov_len: 0,
            iov_base: ptr::null_mut(),
        },
    }
}

unsafe impl<'txn, K> Send for Cursor<'txn, K> where K: TransactionKind {}
unsafe impl<'txn, K> Sync for Cursor<'txn, K> where K: TransactionKind {}

impl<'txn, K> IntoIterator for Cursor<'txn, K>
where
    K: TransactionKind,
{
    type Item = Result<(Cow<'txn, [u8]>, Cow<'txn, [u8]>)>;
    type IntoIter = IntoIter<'txn, K, Cow<'txn, [u8]>, Cow<'txn, [u8]>>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self, MDBX_NEXT, MDBX_NEXT)
    }
}

/// An iterator over the key/value pairs in an MDBX database.
#[derive(Debug)]
pub enum IntoIter<'txn, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// An iterator that returns an error on every call to [Iter::next()].
    /// Cursor.iter*() creates an Iter of this type when MDBX returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Option<Error>),

    /// An iterator that returns an Item on calls to [Iter::next()].
    /// The Item is a [Result], so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The MDBX cursor with which to iterate.
        cursor: Cursor<'txn, K>,

        /// The first operation to perform when the consumer calls [Iter::next()].
        op: ffi::MDBX_cursor_op,

        /// The next and subsequent operations to perform.
        next_op: ffi::MDBX_cursor_op,

        _marker: PhantomData<fn(&'txn (), K, Key, Value)>,
    },
}

impl<'txn, K, Key, Value> IntoIter<'txn, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// Creates a new iterator backed by the given cursor.
    fn new(cursor: Cursor<'txn, K>, op: ffi::MDBX_cursor_op, next_op: ffi::MDBX_cursor_op) -> Self {
        IntoIter::Ok {
            cursor,
            op,
            next_op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, K, Key, Value> Iterator for IntoIter<'txn, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    type Item = Result<(Key, Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Ok {
                cursor,
                op,
                next_op,
                _marker,
            } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, *next_op);
                unsafe {
                    txn_execute(&*cursor.txn, |txn| {
                        match ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) {
                            ffi::MDBX_SUCCESS => {
                                let key = match Key::decode_val::<K>(txn, &key) {
                                    Ok(v) => v,
                                    Err(e) => return Some(Err(e)),
                                };
                                let data = match Value::decode_val::<K>(txn, &data) {
                                    Ok(v) => v,
                                    Err(e) => return Some(Err(e)),
                                };
                                Some(Ok((key, data)))
                            }
                            // MDBX_ENODATA can occur when the cursor was previously seeked to a non-existent value,
                            // e.g. iter_from with a key greater than all values in the database.
                            ffi::MDBX_NOTFOUND | ffi::MDBX_ENODATA => None,
                            error => Some(Err(Error::from_err_code(error))),
                        }
                    })
                }
            }
            Self::Err(err) => err.take().map(Err),
        }
    }
}

/// An iterator over the key/value pairs in an MDBX database.
#[derive(Debug)]
pub enum Iter<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// An iterator that returns an error on every call to [Iter::next()].
    /// Cursor.iter*() creates an Iter of this type when MDBX returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Option<Error>),

    /// An iterator that returns an Item on calls to [Iter::next()].
    /// The Item is a [Result], so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The MDBX cursor with which to iterate.
        cursor: &'cur mut Cursor<'txn, K>,

        /// The first operation to perform when the consumer calls [Iter::next()].
        op: ffi::MDBX_cursor_op,

        /// The next and subsequent operations to perform.
        next_op: ffi::MDBX_cursor_op,

        _marker: PhantomData<fn(&'txn (Key, Value))>,
    },
}

impl<'txn, 'cur, K, Key, Value> Iter<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// Creates a new iterator backed by the given cursor.
    fn new(
        cursor: &'cur mut Cursor<'txn, K>,
        op: ffi::MDBX_cursor_op,
        next_op: ffi::MDBX_cursor_op,
    ) -> Self {
        Iter::Ok {
            cursor,
            op,
            next_op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, 'cur, K, Key, Value> Iterator for Iter<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    type Item = Result<(Key, Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Iter::Ok {
                cursor,
                op,
                next_op,
                ..
            } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, *next_op);
                unsafe {
                    txn_execute(&*cursor.txn, |txn| {
                        match ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) {
                            ffi::MDBX_SUCCESS => {
                                let key = match Key::decode_val::<K>(txn, &key) {
                                    Ok(v) => v,
                                    Err(e) => return Some(Err(e)),
                                };
                                let data = match Value::decode_val::<K>(txn, &data) {
                                    Ok(v) => v,
                                    Err(e) => return Some(Err(e)),
                                };
                                Some(Ok((key, data)))
                            }
                            // MDBX_NODATA can occur when the cursor was previously seeked to a non-existent value,
                            // e.g. iter_from with a key greater than all values in the database.
                            ffi::MDBX_NOTFOUND | ffi::MDBX_ENODATA => None,
                            error => Some(Err(Error::from_err_code(error))),
                        }
                    })
                }
            }
            Iter::Err(err) => err.take().map(Err),
        }
    }
}

/// An iterator over the keys and duplicate values in an MDBX database.
///
/// The yielded items of the iterator are themselves iterators over the duplicate values for a
/// specific key.
pub enum IterDup<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// An iterator that returns an error on every call to Iter.next().
    /// Cursor.iter*() creates an Iter of this type when MDBX returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Option<Error>),

    /// An iterator that returns an Item on calls to Iter.next().
    /// The Item is a Result<(&'txn [u8], &'txn [u8])>, so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The MDBX cursor with which to iterate.
        cursor: &'cur mut Cursor<'txn, K>,

        /// The first operation to perform when the consumer calls Iter.next().
        op: c_uint,

        _marker: PhantomData<fn(&'txn (Key, Value))>,
    },
}

impl<'txn, 'cur, K, Key, Value> IterDup<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    /// Creates a new iterator backed by the given cursor.
    fn new(cursor: &'cur mut Cursor<'txn, K>, op: c_uint) -> Self {
        IterDup::Ok {
            cursor,
            op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, 'cur, K, Key, Value> fmt::Debug for IterDup<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("IterDup").finish()
    }
}

impl<'txn, 'cur, K, Key, Value> Iterator for IterDup<'txn, 'cur, K, Key, Value>
where
    K: TransactionKind,
    Key: TableObject<'txn>,
    Value: TableObject<'txn>,
{
    type Item = IntoIter<'txn, K, Key, Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IterDup::Ok { cursor, op, .. } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, ffi::MDBX_NEXT_NODUP as u32);

                txn_execute(&*cursor.txn, |_| {
                    let err_code =
                        unsafe { ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) };

                    if err_code == ffi::MDBX_SUCCESS {
                        Some(IntoIter::new(
                            Cursor::new_at_position(&**cursor).unwrap(),
                            ffi::MDBX_GET_CURRENT,
                            ffi::MDBX_NEXT_DUP,
                        ))
                    } else {
                        None
                    }
                })
            }
            IterDup::Err(err) => err.take().map(|e| IntoIter::Err(Some(e))),
        }
    }
}
