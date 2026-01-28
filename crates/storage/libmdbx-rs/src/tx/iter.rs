//! Iterator types for traversing MDBX databases.
//!
//! This module provides lending iterators over key-value pairs in MDBX
//! databases. The iterators support both borrowed and owned access patterns.
//!
//! # Iterator Types
//!
//! - [`Iter`]: Base iterator with configurable cursor operation
//! - [`IterKeyVals`]: Iterates over all key-value pairs (`MDBX_NEXT`)
//! - [`IterDupKeys`]: For `DUPSORT` databases, yields first value per key
//! - [`IterDupVals`]: For `DUPSORT` databases, yields all values for one key
//! - [`IterDup`]: Nested iteration over `DUPSORT` databases
//!
//! # Borrowing vs Owning
//!
//! Iterators provide two ways to access data:
//!
//! - [`borrow_next()`](Iter::borrow_next): Returns data potentially borrowed from the database.
//!   Requires the `Key` and `Value` types to implement [`TableObject<'tx>`](crate::TableObject).
//!   This can avoid allocations when using `Cow<'tx, [u8]>`.
//!
//! - [`owned_next()`](Iter::owned_next): Returns owned data. Requires [`TableObjectOwned`]. Always
//!   safe but may allocate.
//!
//! The standard [`Iterator`] trait is implemented via `owned_next()`.
//!
//! # Dirty Page Handling
//!
//! In read-write transactions, database pages may be "dirty" (modified but
//! not yet committed). The behavior of `Cow<[u8]>` depends on the
//! `return-borrowed` feature:
//!
//! - **With `return-borrowed`**: Always returns `Cow::Borrowed`, even for dirty pages. This is
//!   faster but the data may change if the transaction modifies it later.
//!
//! - **Without `return-borrowed`** (default): Dirty pages are copied to `Cow::Owned`. This is safer
//!   but allocates more.
//!
//! # Example
//!
//! ```no_run
//! # use reth_libmdbx::Environment;
//! # use std::path::Path;
//! # let env = Environment::builder().open(Path::new("/tmp/iter_example")).unwrap();
//! let txn = env.begin_ro_txn().unwrap();
//! let db = txn.open_db(None).unwrap();
//! let mut cursor = txn.cursor(db).unwrap();
//!
//! // Iterate using the standard Iterator trait (owned)
//! for result in cursor.iter_start::<Vec<u8>, Vec<u8>>().unwrap() {
//!     let (key, value) = result.expect("decode error");
//!     println!("{:?} => {:?}", key, value);
//! }
//! ```

use crate::{
    error::mdbx_result, tx::TxPtrAccess, Cursor, MdbxError, ReadResult, TableObject,
    TableObjectOwned, TransactionKind,
};
use std::{borrow::Cow, marker::PhantomData, ptr};

/// Iterates over KV pairs in an MDBX database.
pub type IterKeyVals<'tx, 'cur, K, A, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    Iter<'tx, 'cur, K, A, Key, Value, { ffi::MDBX_NEXT }>;

/// An iterator over the key/value pairs in an MDBX `DUPSORT` with duplicate
/// keys, yielding the first value for each key.
///
/// See the [`Iter`] documentation for more details.
pub type IterDupKeys<'tx, 'cur, K, A, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    Iter<'tx, 'cur, K, A, Key, Value, { ffi::MDBX_NEXT_NODUP }>;

/// An iterator over the key/value pairs in an MDBX `DUPSORT`, yielding each
/// duplicate value for a specific key.
pub type IterDupVals<'tx, 'cur, K, A, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    Iter<'tx, 'cur, K, A, Key, Value, { ffi::MDBX_NEXT_DUP }>;

/// An iterator over the key/value pairs in an MDBX database.
///
/// The iteration order is determined by the `OP` const generic parameter.
/// Usually
///
/// This is a lending iterator, meaning that the key and values are borrowed
/// from the underlying cursor when possible. This allows for more efficient
/// iteration without unnecessary allocations, and can be used to create
/// deserializing iterators and other higher-level abstractions.
///
/// Whether borrowing is possible depends on the implementation of
/// [`TableObject`] for both `Key` and `Value`.
pub struct Iter<
    'tx,
    'cur,
    K: TransactionKind,
    A: TxPtrAccess,
    Key = Cow<'tx, [u8]>,
    Value = Cow<'tx, [u8]>,
    const OP: u32 = { ffi::MDBX_NEXT },
> {
    cursor: Cow<'cur, Cursor<'tx, K, A>>,
    /// Pre-fetched value from cursor positioning, yielded before calling FFI.
    pending: Option<(Key, Value)>,
    /// When true, the iterator is exhausted and will always return `None`.
    exhausted: bool,
    _marker: PhantomData<fn() -> (Key, Value)>,
}

impl<K, A, Key, Value, const OP: u32> core::fmt::Debug for Iter<'_, '_, K, A, Key, Value, OP>
where
    K: TransactionKind,
    A: TxPtrAccess,
    Key: core::fmt::Debug,
    Value: core::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Iter").finish()
    }
}

impl<'tx: 'cur, 'cur, K, A, Key, Value, const OP: u32> Iter<'tx, 'cur, K, A, Key, Value, OP>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    /// Create a new iterator from the given cursor, starting at the given
    /// position.
    pub(crate) fn new(cursor: Cow<'cur, Cursor<'tx, K, A>>) -> Self {
        Iter { cursor, pending: None, exhausted: false, _marker: PhantomData }
    }

    /// Create a new iterator from a mutable reference to the given cursor,
    pub(crate) fn from_ref(cursor: &'cur mut Cursor<'tx, K, A>) -> Self {
        Self::new(Cow::Borrowed(cursor))
    }

    /// Create a new iterator that is already exhausted.
    ///
    /// Iteration will immediately return `None`.
    pub(crate) fn new_end(cursor: Cow<'cur, Cursor<'tx, K, A>>) -> Self {
        Iter { cursor, pending: None, exhausted: true, _marker: PhantomData }
    }

    /// Create a new, exhausted iterator from a mutable reference to the given
    /// cursor. This is usually used as a placeholder when no items are to be
    /// yielded.
    pub(crate) fn end_from_ref(cursor: &'cur mut Cursor<'tx, K, A>) -> Self {
        Self::new_end(Cow::Borrowed(cursor))
    }

    /// Create a new iterator from the given cursor, first yielding the
    /// provided key/value pair.
    pub(crate) fn new_with(cursor: Cow<'cur, Cursor<'tx, K, A>>, first: (Key, Value)) -> Self {
        Iter { cursor, pending: Some(first), exhausted: false, _marker: PhantomData }
    }

    /// Create a new iterator from a mutable reference to the given cursor,
    /// first yielding the provided key/value pair.
    pub(crate) fn from_ref_with(cursor: &'cur mut Cursor<'tx, K, A>, first: (Key, Value)) -> Self {
        Self::new_with(Cow::Borrowed(cursor), first)
    }

    /// Create a new iterator from an owned cursor, first yielding the
    /// provided key/value pair.
    pub(crate) fn from_owned_with(cursor: Cursor<'tx, K, A>, first: (Key, Value)) -> Self
    where
        A: Sized,
    {
        Self::new_with(Cow::Owned(cursor), first)
    }
}

impl<K, A, Key, Value, const OP: u32> Iter<'_, '_, K, A, Key, Value, OP>
where
    K: TransactionKind,
    A: TxPtrAccess,
    Key: TableObjectOwned,
    Value: TableObjectOwned,
{
    /// Own the next key/value pair from the iterator.
    pub fn owned_next(&mut self) -> ReadResult<Option<(Key, Value)>> {
        if self.exhausted {
            return Ok(None);
        }
        if let Some(v) = self.pending.take() {
            return Ok(Some(v));
        }
        self.execute_op()
    }
}

impl<'tx: 'cur, 'cur, K, A, Key, Value, const OP: u32> Iter<'tx, 'cur, K, A, Key, Value, OP>
where
    K: TransactionKind,
    A: TxPtrAccess,
    Key: TableObject<'tx>,
    Value: TableObject<'tx>,
{
    /// Execute the MDBX operation and decode the result.
    ///
    /// Returns `Ok(Some((key, value)))` if a key/value pair was found,
    /// `Ok(None)` if no more key/value pairs are available, or `Err` on error.
    fn execute_op(&self) -> ReadResult<Option<(Key, Value)>> {
        let mut key = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };
        let mut data = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

        self.cursor.access().with_txn_ptr(|txn| {
            let res =
                unsafe { ffi::mdbx_cursor_get(self.cursor.cursor(), &mut key, &mut data, OP) };

            match res {
                ffi::MDBX_SUCCESS => {
                    // SAFETY: decode_val checks for dirty writes and copies if needed.
                    // The lifetime 'tx guarantees the Cow cannot outlive the transaction.
                    unsafe {
                        let key = TableObject::decode_val::<K>(txn, key)?;
                        let data = TableObject::decode_val::<K>(txn, data)?;
                        Ok(Some((key, data)))
                    }
                }
                ffi::MDBX_NOTFOUND | ffi::MDBX_ENODATA | ffi::MDBX_RESULT_TRUE => Ok(None),
                other => Err(MdbxError::from_err_code(other).into()),
            }
        })?
    }

    /// Borrow the next key/value pair from the iterator.
    ///
    /// Returns `Ok(Some((key, value)))` if a key/value pair was found,
    /// `Ok(None)` if no more key/value pairs are available, or `Err` on DB
    /// access error.
    pub fn borrow_next(&mut self) -> ReadResult<Option<(Key, Value)>> {
        if self.exhausted {
            return Ok(None);
        }
        if let Some(v) = self.pending.take() {
            return Ok(Some(v));
        }
        self.execute_op()
    }
}

impl<K, A, Key, Value, const OP: u32> Iterator for Iter<'_, '_, K, A, Key, Value, OP>
where
    K: TransactionKind,
    A: TxPtrAccess,
    Key: TableObjectOwned,
    Value: TableObjectOwned,
{
    type Item = ReadResult<(Key, Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        self.owned_next().transpose()
    }
}

/// An iterator over the key/value pairs in an MDBX database with duplicate
/// keys.
pub struct IterDup<
    'tx,
    'cur,
    K: TransactionKind,
    A: TxPtrAccess,
    Key = Cow<'tx, [u8]>,
    Value = Cow<'tx, [u8]>,
> {
    inner: IterDupKeys<'tx, 'cur, K, A, Key, Value>,
}

impl<'tx, 'cur, K, A, Key, Value> core::fmt::Debug for IterDup<'tx, 'cur, K, A, Key, Value>
where
    K: TransactionKind,
    A: TxPtrAccess,
    Key: core::fmt::Debug,
    Value: core::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IterDup").finish()
    }
}

impl<'tx, 'cur, K, A, Key, Value> IterDup<'tx, 'cur, K, A, Key, Value>
where
    K: TransactionKind,
    A: TxPtrAccess,
{
    /// Create a new iterator from the given cursor, starting at the given
    /// position.
    pub(crate) fn new(cursor: Cow<'cur, Cursor<'tx, K, A>>) -> Self {
        IterDup { inner: IterDupKeys::new(cursor) }
    }

    /// Create a new iterator from a mutable reference to the given cursor,
    pub(crate) fn from_ref(cursor: &'cur mut Cursor<'tx, K, A>) -> Self {
        Self::new(Cow::Borrowed(cursor))
    }

    /// Create a new iterator from an owned cursor.
    pub fn from_owned(cursor: Cursor<'tx, K, A>) -> Self
    where
        A: Sized,
    {
        Self::new(Cow::Owned(cursor))
    }

    /// Create a new iterator from the given cursor, the inner iterator will
    /// first yield the provided key/value pair.
    pub(crate) fn new_with(cursor: Cow<'cur, Cursor<'tx, K, A>>, first: (Key, Value)) -> Self {
        IterDup { inner: Iter::new_with(cursor, first) }
    }

    /// Create a new iterator from a mutable reference to the given cursor,
    /// first yielding the provided key/value pair.
    pub fn from_ref_with(cursor: &'cur mut Cursor<'tx, K, A>, first: (Key, Value)) -> Self {
        Self::new_with(Cow::Borrowed(cursor), first)
    }

    /// Create a new iterator from the given cursor, with no items to yield.
    pub fn new_end(cursor: Cow<'cur, Cursor<'tx, K, A>>) -> Self {
        IterDup { inner: Iter::new_end(cursor) }
    }

    /// Create a new iterator from a mutable reference to the given cursor, with
    /// no items to yield.
    pub fn end_from_ref(cursor: &'cur mut Cursor<'tx, K, A>) -> Self {
        Self::new_end(Cow::Borrowed(cursor))
    }

    /// Create a new iterator from an owned cursor, with no items to yield.
    pub fn end_from_owned(cursor: Cursor<'tx, K, A>) -> Self
    where
        A: Sized,
    {
        Self::new_end(Cow::Owned(cursor))
    }
}

impl<'tx: 'cur, 'cur, K, A, Key, Value> IterDup<'tx, 'cur, K, A, Key, Value>
where
    K: TransactionKind,
    A: TxPtrAccess + 'tx,
    Key: TableObject<'tx>,
    Value: TableObject<'tx>,
{
    /// Borrow the next key/value pair from the iterator.
    pub fn borrow_next(&mut self) -> ReadResult<Option<IterDupVals<'tx, 'cur, K, A, Key, Value>>> {
        // We want to use Cursor::new_at_position to create a new cursor,
        // but the kv pair may be borrowed from the inner cursor, so we need to
        // store the references first. This is just to avoid borrow checker
        // issues in the unsafe block.
        let cursor_ptr = self.inner.cursor.as_ref().cursor();

        // SAFETY: the access lives as long as self.inner.cursor, and the cursor op
        // we perform does not invalidate the data borrowed from the inner
        // cursor in borrow_next.
        let access: *const A = self.inner.cursor.access();
        let access = unsafe { access.as_ref().unwrap() };

        // The next will be the FIRST KV pair for the NEXT key in the DUPSORT
        match self.inner.borrow_next()? {
            Some((key, value)) => {
                // SAFETY: the access is valid as per above. The FFI calls here do
                // not invalidate any data borrowed from the inner cursor.
                //
                // This is an inlined version of Cursor::new_at_position.
                let db = self.inner.cursor.as_ref().db();
                let dup_cursor = access.with_txn_ptr(move |_| unsafe {
                    let new_cursor = ffi::mdbx_cursor_create(ptr::null_mut());
                    let res = ffi::mdbx_cursor_copy(cursor_ptr, new_cursor);
                    mdbx_result(res)?;
                    Ok::<_, MdbxError>(Cursor::new_raw(access, new_cursor, db))
                })??;

                Ok(Some(IterDupVals::from_owned_with(dup_cursor, (key, value))))
            }
            None => Ok(None),
        }
    }
}

impl<'tx: 'cur, 'cur, K, A, Key, Value> IterDup<'tx, 'cur, K, A, Key, Value>
where
    K: TransactionKind,
    A: TxPtrAccess + 'tx,
    Key: TableObjectOwned,
    Value: TableObjectOwned,
{
    /// Own the next key/value pair from the iterator.
    pub fn owned_next(&mut self) -> ReadResult<Option<IterDupVals<'tx, 'cur, K, A, Key, Value>>> {
        self.borrow_next()
    }
}

impl<'tx: 'cur, 'cur, K, A, Key, Value> Iterator for IterDup<'tx, 'cur, K, A, Key, Value>
where
    K: TransactionKind,
    A: TxPtrAccess + 'tx,
    Key: TableObjectOwned,
    Value: TableObjectOwned,
{
    type Item = ReadResult<IterDupVals<'tx, 'cur, K, A, Key, Value>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.owned_next().transpose()
    }
}

/// A key-value iterator for a synchronized read-only transaction.
pub type RoIterSync<'tx, 'cur, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    IterKeyVals<'tx, 'cur, crate::RO, crate::tx::PtrSyncInner<crate::RO>, Key, Value>;

/// A key-value iterator for a synchronized read-write transaction.
pub type RwIterSync<'tx, 'cur, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    IterKeyVals<'tx, 'cur, crate::RW, crate::tx::PtrSyncInner<crate::RW>, Key, Value>;

/// A key-value iterator for an unsynchronized read-only transaction.
pub type RoIterUnsync<'tx, 'cur, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    IterKeyVals<'tx, 'cur, crate::RO, crate::tx::RoGuard, Key, Value>;

/// A key-value iterator for an unsynchronized read-write transaction.
pub type RwIterUnsync<'tx, 'cur, Key = Cow<'tx, [u8]>, Value = Cow<'tx, [u8]>> =
    IterKeyVals<'tx, 'cur, crate::RW, crate::tx::RwUnsync, Key, Value>;
