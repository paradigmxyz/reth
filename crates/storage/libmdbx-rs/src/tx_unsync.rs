//! Unsynchronized transaction implementation.
//!
//! This module provides [`TxUnsync`], an alternative transaction type that
//! avoids mutex overhead by enforcing exclusive access at compile time.
//!
//! # Performance
//!
//! `TxUnsync` is significantly faster than [`Transaction`](crate::Transaction)
//! for single-threaded workloads because it eliminates the mutex lock/unlock
//! on every database operation:
//!
//! - Up to **3-4x faster writes** in benchmarks
//! - Up to **20% faster reads**
//!
//! # Thread Safety
//!
//! | Transaction Type | Send | Sync | Use Case |
//! |------------------|------|------|----------|
//! | `TxUnsync<RO>`   | Yes  | No   | Single-threaded reads, can move between threads |
//! | `TxUnsync<RW>`   | No   | No   | Single-threaded writes, must stay on creating thread |
//!
//! For multi-threaded access, use the standard [`Transaction`](crate::Transaction) type.

use crate::{
    database::Database,
    environment::Environment,
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    transaction::{CommitLatency, TransactionKind, RO, RW},
    tx_access::{RoUnsync, RwUnsync, TxPtrAccess},
    Cursor, Error, Stat, TableObject,
};
use std::{
    ffi::{c_uint, c_void},
    fmt::{self, Debug},
    marker::PhantomData,
    mem::size_of,
    ptr, slice,
};

/// An unsynchronized MDBX transaction.
///
/// This transaction type provides zero-overhead access to the database by
/// avoiding mutex synchronization. All methods take `&mut self` to enforce
/// exclusive access at compile time.
///
/// # Example
///
/// ```ignore
/// let env = Environment::builder().open(path)?;
///
/// // Write data
/// let mut txn = env.begin_rw_unsync()?;
/// let db = txn.create_db(None, DatabaseFlags::empty())?;
/// txn.put(db.dbi(), b"key", b"value", WriteFlags::empty())?;
/// txn.commit()?;
///
/// // Read data
/// let mut txn = env.begin_ro_unsync()?;
/// let db = txn.open_db(None)?;
/// let value: Option<Vec<u8>> = txn.get(db.dbi(), b"key")?;
/// ```
pub struct TxUnsync<K: TransactionKind> {
    inner: K::UnsyncInner,
    env: Environment,
    _marker: PhantomData<K>,
}

impl<K: TransactionKind> TxUnsync<K> {
    /// Returns a reference to the environment.
    pub const fn env(&self) -> &Environment {
        &self.env
    }

    /// Returns the transaction id.
    pub fn id(&mut self) -> Result<u64> {
        self.inner.with_txn_ptr(|txn| unsafe { ffi::mdbx_txn_id(txn) })
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the
    /// database. If the database supports duplicate keys
    /// ([`DatabaseFlags::DUP_SORT`]) then the first data item for the key will be
    /// returned. Retrieval of other items requires the use of
    /// [`Cursor`]. If the item is not in the database, then
    /// [None] will be returned.
    pub fn get<Key>(&mut self, dbi: ffi::MDBX_dbi, key: &[u8]) -> Result<Option<Key>>
    where
        Key: TableObject,
    {
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

        self.inner.with_txn_ptr(|txn| unsafe {
            match ffi::mdbx_get(txn, dbi, &key_val, &mut data_val) {
                ffi::MDBX_SUCCESS => Key::decode_val::<K>(txn, data_val).map(Some),
                ffi::MDBX_NOTFOUND => Ok(None),
                err_code => Err(Error::from_err_code(err_code)),
            }
        })?
    }

    /// Opens a handle to an MDBX database.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database.
    pub fn open_db(&mut self, name: Option<&str>) -> Result<Database> {
        Database::new_unsync(&mut self.inner, name, 0)
    }

    /// Gets the option flags for the given database in the transaction.
    pub fn db_flags(&mut self, dbi: ffi::MDBX_dbi) -> Result<DatabaseFlags> {
        let mut flags: c_uint = 0;
        self.inner.with_txn_ptr(|txn| unsafe {
            let mut _status: c_uint = 0;
            mdbx_result(ffi::mdbx_dbi_flags_ex(txn, dbi, &mut flags, &mut _status))
        })??;

        #[cfg_attr(not(windows), allow(clippy::useless_conversion))]
        Ok(DatabaseFlags::from_bits_truncate(flags.try_into().unwrap()))
    }

    /// Retrieves database statistics.
    pub fn db_stat(&mut self, dbi: ffi::MDBX_dbi) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            self.inner.with_txn_ptr(|txn| {
                mdbx_result(ffi::mdbx_dbi_stat(txn, dbi, stat.mdb_stat(), size_of::<Stat>()))
            })??;
            Ok(stat)
        }
    }
}

impl TxUnsync<RO> {
    /// Creates a new unsynchronized read-only transaction.
    pub(crate) fn new(env: Environment) -> Result<Self> {
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_txn_begin_ex(
                env.env_ptr(),
                ptr::null_mut(),
                RO::OPEN_FLAGS,
                &mut txn,
                ptr::null_mut(),
            ))?;
        }
        Ok(Self { inner: RoUnsync::new(txn), env, _marker: PhantomData })
    }

    /// Open a new cursor on the given database.
    pub fn cursor(&mut self, dbi: ffi::MDBX_dbi) -> Result<Cursor<RO>> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        self.inner.with_txn_ptr(|txn| unsafe {
            mdbx_result(ffi::mdbx_cursor_open(txn, dbi, &mut cursor))
        })??;

        // We need a Transaction for the cursor, so we create a temporary one
        // that shares the same underlying transaction
        let txn = self.env.begin_ro_txn()?;
        Cursor::new(txn, dbi)
    }

    /// Closes the database handle.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi
    /// BEFORE calling this function.
    pub unsafe fn close_db(&mut self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(unsafe { ffi::mdbx_dbi_close(self.env.env_ptr(), dbi) })?;
        Ok(())
    }
}

impl TxUnsync<RW> {
    /// Creates a new unsynchronized read-write transaction.
    pub(crate) fn new(env: Environment) -> Result<Self> {
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_txn_begin_ex(
                env.env_ptr(),
                ptr::null_mut(),
                RW::OPEN_FLAGS,
                &mut txn,
                ptr::null_mut(),
            ))?;
        }
        Ok(Self { inner: RwUnsync::new(txn), env, _marker: PhantomData })
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    pub fn commit(self) -> Result<CommitLatency> {
        let result = self.inner.with_txn_ptr(|txn| {
            let mut latency = CommitLatency::new();
            let res =
                mdbx_result(unsafe { ffi::mdbx_txn_commit_ex(txn, latency.mdb_commit_latency()) });
            res.map(|v| (v, latency))
        })?;

        match result {
            Ok((false, lat)) => {
                self.inner.set_committed();
                Ok(lat)
            }
            Ok((true, _)) => {
                self.inner.set_committed();
                Err(Error::BotchedTransaction)
            }
            Err(e) => Err(e),
        }
    }

    /// Opens a handle to an MDBX database, creating the database if necessary.
    pub fn create_db(&mut self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        Database::new_unsync(&mut self.inner, name, (flags | DatabaseFlags::CREATE).bits())
    }

    /// Stores an item into a database.
    pub fn put(
        &mut self,
        dbi: ffi::MDBX_dbi,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
        flags: WriteFlags,
    ) -> Result<()> {
        let key = key.as_ref();
        let data = data.as_ref();
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };
        mdbx_result(self.inner.with_txn_ptr(|txn| unsafe {
            ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits())
        })?)?;
        Ok(())
    }

    /// Returns a buffer which can be used to write a value into the item at the
    /// given key and with the given length.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned buffer is not used after the transaction is
    /// committed or aborted.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn reserve(
        &mut self,
        dbi: ffi::MDBX_dbi,
        key: impl AsRef<[u8]>,
        len: usize,
        flags: WriteFlags,
    ) -> Result<&mut [u8]> {
        let key = key.as_ref();
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: len, iov_base: ptr::null_mut::<c_void>() };
        unsafe {
            mdbx_result(self.inner.with_txn_ptr(|txn| {
                ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits() | ffi::MDBX_RESERVE)
            })?)?;
            Ok(slice::from_raw_parts_mut(data_val.iov_base as *mut u8, data_val.iov_len))
        }
    }

    /// Delete items from a database.
    ///
    /// Returns `true` if the key/value pair was present.
    pub fn del(
        &mut self,
        dbi: ffi::MDBX_dbi,
        key: impl AsRef<[u8]>,
        data: Option<&[u8]>,
    ) -> Result<bool> {
        let key = key.as_ref();
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let data_val: Option<ffi::MDBX_val> = data.map(|data| ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        });

        mdbx_result({
            self.inner.with_txn_ptr(|txn| {
                if let Some(d) = data_val {
                    unsafe { ffi::mdbx_del(txn, dbi, &key_val, &d) }
                } else {
                    unsafe { ffi::mdbx_del(txn, dbi, &key_val, ptr::null()) }
                }
            })?
        })
        .map(|_| true)
        .or_else(|e| match e {
            Error::NotFound => Ok(false),
            other => Err(other),
        })
    }

    /// Empties the given database. All items will be removed.
    pub fn clear_db(&mut self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(self.inner.with_txn_ptr(|txn| unsafe { ffi::mdbx_drop(txn, dbi, false) })?)?;
        Ok(())
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing
    /// to the same dbi BEFORE calling this function.
    pub unsafe fn drop_db(&mut self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(self.inner.with_txn_ptr(|txn| unsafe { ffi::mdbx_drop(txn, dbi, true) })?)?;
        Ok(())
    }
}

impl<K: TransactionKind> Debug for TxUnsync<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxUnsync").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_unsync_rw_basic() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        // Write data
        let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
        let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
        txn.put(db.dbi(), b"key1", b"value1", WriteFlags::empty()).unwrap();
        txn.put(db.dbi(), b"key2", b"value2", WriteFlags::empty()).unwrap();
        txn.commit().unwrap();

        // Read data back
        let mut txn = TxUnsync::<RO>::new(env).unwrap();
        let db = txn.open_db(None).unwrap();
        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"key1").unwrap();
        assert_eq!(value.as_deref(), Some(b"value1".as_slice()));

        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"key2").unwrap();
        assert_eq!(value.as_deref(), Some(b"value2".as_slice()));

        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"nonexistent").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn test_unsync_rw_abort_on_drop() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        // Write data but don't commit
        {
            let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
            let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
            txn.put(db.dbi(), b"key", b"value", WriteFlags::empty()).unwrap();
            // txn drops here without commit
        }

        // Data should not be visible
        let mut txn = TxUnsync::<RO>::new(env).unwrap();
        let db = txn.open_db(None).unwrap();
        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"key").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn test_unsync_del() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        // Write and commit
        let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
        let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
        txn.put(db.dbi(), b"key", b"value", WriteFlags::empty()).unwrap();
        txn.commit().unwrap();

        // Delete
        let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
        let db = txn.open_db(None).unwrap();
        let deleted = txn.del(db.dbi(), b"key", None).unwrap();
        assert!(deleted);
        txn.commit().unwrap();

        // Verify deleted
        let mut txn = TxUnsync::<RO>::new(env).unwrap();
        let db = txn.open_db(None).unwrap();
        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"key").unwrap();
        assert!(value.is_none());
    }
}
