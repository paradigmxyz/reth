//! Alternative transaction implementation that provide unsynchronized
//! access for read-write transactions.
//!
//! These transactions are significantly faster than synchronized transactions
//! when used in a single-threaded context, as they avoid the overhead of
//! mutexes locking. However, they are `!Sync` due to MDBX's thread
//! requirements. The RW transactions are `!Send` and must be used on the
//! creating thread, while RO transactions can be sent between threads but not
//! shared concurrently.
//!
//! | Transaction Type | Send | Sync | MDBX Requirement              |
//! |------------------|------|------|-------------------------------|
//! | Read-Only (RO)   | Yes  | No   | Total ordering of access      |
//! | Read-Write (RW)  | No   | No   | Single-threaded ownership     |
//!
//! # Design
//!
//! - **RO transactions**: Use Arc/Weak pattern for timeout support. When a timeout is set, a
//!   background thread holds the Arc and drops it on timeout, causing the transaction to be
//!   aborted. The transaction holds a Weak reference and upgrades it for each operation.
//!
//! - **RW transactions**: Use direct pointer ownership with `!Send` to ensure they stay on the
//!   creating thread. No mutex needed since RW transactions are single-threaded.

#[cfg(debug_assertions)]
use crate::tx::assertions;
use crate::{
    error::{mdbx_result, MdbxResult},
    flags::{DatabaseFlags, WriteFlags},
    tx::{
        access::RoTxPtr,
        cache::{CachedDb, DbCache},
        ops, Cursor, RoGuard, RwUnsync, TxPtrAccess,
    },
    CommitLatency, Database, Environment, MdbxError, ReadResult, Stat, TableObject,
    TransactionKind, RO, RW,
};
use smallvec::SmallVec;
use std::{ffi::CStr, fmt, marker::PhantomData, ptr};

/// Meta-data for a transaction.
pub struct TxMeta {
    env: Environment,
    db_cache: DbCache,
    span: tracing::Span,
}

impl fmt::Debug for TxMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxMeta").finish()
    }
}

/// A database transaction (v2 implementation).
///
/// This implementation uses:
/// - Arc/Weak pattern for RO transactions (!Sync, with timeout support)
/// - Direct ownership for RW transactions (!Send, !Sync, no mutex needed)
pub struct TxUnsync<K: TransactionKind> {
    txn: K::Inner,

    meta: TxMeta,

    _marker: PhantomData<K>,
}

impl<K: TransactionKind> TxUnsync<K> {
    fn new_inner(env: Environment) -> MdbxResult<(*mut ffi::MDBX_txn, TxMeta)> {
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        // SAFETY: env.env_ptr() is valid, we check the result.
        unsafe {
            mdbx_result(ffi::mdbx_txn_begin_ex(
                env.env_ptr(),
                ptr::null_mut(),
                K::OPEN_FLAGS,
                &mut txn,
                ptr::null_mut(),
            ))?;
        }

        let txn_id = unsafe { ffi::mdbx_txn_id(txn) };
        let span = tracing::debug_span!(
            target: "libmdbx",
            "mdbx_txn_v2",
            kind = if K::IS_READ_ONLY { "ro" } else { "rw" },
            txn_id,
            committed = false,
        );

        Ok((txn, TxMeta { env, db_cache: DbCache::default(), span }))
    }
}

impl TxUnsync<RO> {
    /// Creates the raw pointer and metadata from an environment.
    fn begin(env: Environment) -> MdbxResult<(RoTxPtr, TxMeta)> {
        let (txn, meta) = Self::new_inner(env)?;
        Ok((RoTxPtr::from(txn), meta))
    }

    /// Completes construction from a guard and metadata.
    const fn from_guard(guard: RoGuard, meta: TxMeta) -> Self {
        Self { txn: guard, meta, _marker: PhantomData }
    }

    /// Creates a new read-only transaction.
    #[cfg(not(feature = "read-tx-timeouts"))]
    pub(crate) fn new(env: Environment) -> MdbxResult<Self> {
        let (ptr, meta) = Self::begin(env)?;
        Ok(Self::from_guard(RoGuard::new_no_timeout(ptr), meta))
    }

    /// Creates a new read-only transaction with the default timeout.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn new(env: Environment) -> MdbxResult<Self> {
        use crate::tx::DEFAULT_MAX_READ_TRANSACTION_DURATION;
        Self::new_with_timeout(env, DEFAULT_MAX_READ_TRANSACTION_DURATION)
    }

    /// Creates a new read-only transaction without a timeout.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn new_no_timeout(env: Environment) -> MdbxResult<Self> {
        let (ptr, meta) = Self::begin(env)?;
        Ok(Self::from_guard(RoGuard::new_no_timeout(ptr), meta))
    }

    /// Creates a new read-only transaction with a custom timeout.
    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn new_with_timeout(
        env: Environment,
        duration: std::time::Duration,
    ) -> MdbxResult<Self> {
        let (ptr, meta) = Self::begin(env)?;
        Ok(Self::from_guard(RoGuard::new_with_timeout(ptr, duration), meta))
    }

    /// Tries to disable the timeout timer for this transaction.
    #[cfg(feature = "read-tx-timeouts")]
    pub fn try_disable_timer(&mut self) -> MdbxResult<()> {
        self.txn.try_disable_timer()
    }
}

impl TxUnsync<RW> {
    /// Creates a new read-write transaction.
    pub(crate) fn new(env: Environment) -> MdbxResult<Self> {
        let (txn, meta) = Self::new_inner(env)?;

        let ptr = RwUnsync::new(txn);

        Ok(Self { txn: ptr, meta, _marker: PhantomData })
    }
}

impl<K: TransactionKind> TxUnsync<K> {
    /// Gets the raw transaction pointer
    ///
    /// This transaction takes &mut self to ensure exclusive access. This
    /// ensures that accesses are serialized by definition, without needing a
    /// mutex or other synchronization primitive.
    #[inline(always)]
    fn with_txn_ptr<F, R>(&mut self, f: F) -> MdbxResult<R>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> R,
    {
        self.txn.with_txn_ptr(f)
    }

    /// Returns a reference to the environment.
    pub const fn env(&self) -> &Environment {
        &self.meta.env
    }

    /// Returns the tracing span for this transaction.
    pub const fn span(&self) -> &tracing::Span {
        &self.meta.span
    }

    /// Returns the transaction id.
    pub fn id(&mut self) -> MdbxResult<u64> {
        self.with_txn_ptr(|txn_ptr| Ok(unsafe { ffi::mdbx_txn_id(txn_ptr) }))?
    }

    /// Gets an item from a database.
    pub fn get<'a, Key>(&'a mut self, dbi: ffi::MDBX_dbi, key: &[u8]) -> ReadResult<Option<Key>>
    where
        Key: TableObject<'a>,
    {
        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe {
                let data_val = ops::get_raw(txn_ptr, dbi, key)?;
                data_val.map(|val| Key::decode_val::<K>(txn_ptr, val)).transpose()
            }
        })?
    }

    /// Opens a handle to an MDBX database.
    pub fn open_db(&mut self, name: Option<&str>) -> MdbxResult<Database> {
        let name_hash = CachedDb::hash_name(name);

        if let Some(db) = self.meta.db_cache.read_db(name_hash) {
            return Ok(db);
        }

        self.open_and_cache_with_flags(name, DatabaseFlags::empty()).map(Into::into)
    }

    /// Opens a database handle without using the cache.
    pub fn open_db_no_cache(&mut self, name: Option<&str>) -> MdbxResult<Database> {
        self.open_db_with_flags(name, DatabaseFlags::empty()).map(Into::into)
    }

    fn open_and_cache_with_flags(
        &mut self,
        name: Option<&str>,
        flags: DatabaseFlags,
    ) -> MdbxResult<CachedDb> {
        let db = self.open_db_with_flags(name, flags)?;
        self.meta.db_cache.write_db(db);
        Ok(db)
    }

    fn open_db_with_flags(
        &mut self,
        name: Option<&str>,
        flags: DatabaseFlags,
    ) -> MdbxResult<CachedDb> {
        let mut c_name_buf = SmallVec::<[u8; 32]>::new();
        let c_name = name.map(|n| {
            c_name_buf.extend_from_slice(n.as_bytes());
            c_name_buf.push(0);
            CStr::from_bytes_with_nul(&c_name_buf).unwrap()
        });
        let name_ptr = c_name.as_ref().map_or(ptr::null(), |s| s.as_ptr());

        let (dbi, db_flags) = self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr, name_ptr is valid or null.
            unsafe { ops::open_db_raw(txn_ptr, name_ptr, flags) }
        })??;

        Ok(CachedDb::new(name, Database::new(dbi, db_flags)))
    }

    /// Gets the option flags for the given database.
    pub fn db_flags(&mut self, dbi: ffi::MDBX_dbi) -> MdbxResult<DatabaseFlags> {
        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::db_flags_raw(txn_ptr, dbi) }
        })?
    }

    /// Retrieves database statistics.
    pub fn db_stat(&mut self, dbi: ffi::MDBX_dbi) -> MdbxResult<Stat> {
        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::db_stat_raw(txn_ptr, dbi) }
        })?
    }

    /// Opens a cursor on the given database.
    ///
    /// Multiple cursors can be open simultaneously on different databases
    /// within the same transaction. The cursor borrows the transaction's
    /// inner access type, allowing concurrent cursor operations.
    pub fn cursor(&self, db: Database) -> MdbxResult<Cursor<'_, K, K::Inner>> {
        Cursor::new(&self.txn, db)
    }

    /// Commits the transaction (inner implementation).
    fn commit_inner(mut self, latency: *mut ffi::MDBX_commit_latency) -> MdbxResult<()> {
        // Self is dropped at end of function, so RwTxPtr::drop will be within
        // span scope.
        let _guard = self.meta.span.clone().entered();

        // SAFETY: txn_ptr is valid from with_txn_ptr.
        let was_aborted =
            self.with_txn_ptr(|txn_ptr| unsafe { ops::commit_raw(txn_ptr, latency) })??;

        self.txn.mark_committed();

        if was_aborted {
            tracing::warn!(target: "libmdbx", "botched");
            return Err(MdbxError::BotchedTransaction);
        }

        Ok(())
    }

    /// Commit the transaction.
    ///
    /// For RO transactions, this will release resources held by the
    /// transaction. For RW transactions, this will persist changes to the
    /// database.
    pub fn commit(self) -> MdbxResult<()> {
        // SAFETY: txn_ptr is valid.
        self.commit_inner(std::ptr::null_mut())
    }

    /// Commits the transaction and returns commit latency information.
    ///
    /// For RO transactions, this will release resources held by the
    /// transaction. For RW transactions, this will persist changes to the
    /// database.
    pub fn commit_with_latency(self) -> MdbxResult<CommitLatency> {
        let mut latency = CommitLatency::new();
        self.commit_inner(latency.mdb_commit_latency())?;
        Ok(latency)
    }
}

impl TxUnsync<RW> {
    /// Creates a database if it doesn't exist.
    pub fn create_db(&mut self, name: Option<&str>, flags: DatabaseFlags) -> MdbxResult<Database> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE).map(Into::into)
    }

    /// Stores an item into a database.
    pub fn put(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
        flags: WriteFlags,
    ) -> MdbxResult<()> {
        let key = key.as_ref();
        let data = data.as_ref();

        #[cfg(debug_assertions)]
        {
            let pagesize = self.env().stat().map(|s| s.page_size() as usize).unwrap_or(4096);
            assertions::debug_assert_put(pagesize, db.flags(), key, data);
        }

        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::put_raw(txn_ptr, db.dbi(), key, data, flags) }
        })?
    }

    /// Appends a key/data pair to the end of the database.
    ///
    /// The key must be greater than all existing keys (or less than, for
    /// [`DatabaseFlags::REVERSE_KEY`] tables). This is more efficient than
    /// [`TxUnsync::put`] when adding data in sorted order.
    ///
    /// In debug builds, this method asserts that the key ordering constraint is
    /// satisfied.
    pub fn append(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
    ) -> MdbxResult<()> {
        let key = key.as_ref();
        let data = data.as_ref();

        self.with_txn_ptr(|txn_ptr| {
            #[cfg(debug_assertions)]
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe {
                ops::debug_assert_append(txn_ptr, db.dbi(), db.flags(), key, data);
            }

            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::put_raw(txn_ptr, db.dbi(), key, data, WriteFlags::APPEND) }
        })?
    }

    /// Appends duplicate data for [`DatabaseFlags::DUP_SORT`] databases.
    ///
    /// The data must be greater than all existing data for this key (or less
    /// than, for [`DatabaseFlags::REVERSE_DUP`] tables). This is more efficient
    /// than [`TxUnsync::put`] when adding duplicates in sorted order.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    ///
    /// In debug builds, this method asserts that the data ordering constraint
    /// is satisfied.
    pub fn append_dup(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
    ) -> MdbxResult<()> {
        if !db.flags().contains(DatabaseFlags::DUP_SORT) {
            return Err(MdbxError::RequiresDupSort);
        }
        let key = key.as_ref();
        let data = data.as_ref();

        self.with_txn_ptr(|txn_ptr| {
            #[cfg(debug_assertions)]
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe {
                ops::debug_assert_append_dup(txn_ptr, db.dbi(), db.flags(), key, data);
            }

            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::put_raw(txn_ptr, db.dbi(), key, data, WriteFlags::APPEND_DUP) }
        })?
    }

    /// Reserves space for a value and returns a mutable slice to write into.
    ///
    /// # Safety
    ///
    /// The returned buffer is only valid until another value is inserted or
    /// the transaction is committed/aborted.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn reserve(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        len: usize,
        flags: WriteFlags,
    ) -> MdbxResult<&mut [u8]> {
        let key = key.as_ref();

        #[cfg(debug_assertions)]
        {
            let pagesize = self.env().stat().map(|s| s.page_size() as usize).unwrap_or(4096);
            assertions::debug_assert_key(pagesize, db.flags(), key);
        }

        let ptr = self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::reserve_raw(txn_ptr, db.dbi(), key, len, flags) }
        })??;
        // SAFETY: ptr is valid from reserve_raw, len matches.
        Ok(unsafe { ops::slice_from_reserved(ptr, len) })
    }

    /// Reserves space and calls the closure to write into it.
    pub fn with_reservation(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        len: usize,
        flags: WriteFlags,
        f: impl FnOnce(&mut [u8]),
    ) -> MdbxResult<()> {
        // SAFETY: We ensure the buffer is written to before any other operation.
        let buf = unsafe { self.reserve(db, key, len, flags)? };
        f(buf);
        Ok(())
    }

    /// Deletes items from a database.
    pub fn del(
        &mut self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: Option<&[u8]>,
    ) -> MdbxResult<bool> {
        let key = key.as_ref();

        #[cfg(debug_assertions)]
        {
            let pagesize = self.env().stat().map(|s| s.page_size() as usize).unwrap_or(4096);
            assertions::debug_assert_key(pagesize, db.flags(), key);
            if let Some(v) = data {
                assertions::debug_assert_value(pagesize, db.flags(), v);
            }
        }

        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::del_raw(txn_ptr, db.dbi(), key, data) }
        })?
    }

    /// Empties the given database.
    pub fn clear_db(&mut self, db: Database) -> MdbxResult<()> {
        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: txn_ptr is valid from with_txn_ptr.
            unsafe { ops::clear_db_raw(txn_ptr, db.dbi()) }
        })?
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    ///
    /// Caller must ensure all other Database and Cursor instances pointing
    /// to this dbi are closed before calling.
    pub unsafe fn drop_db(&mut self, db: Database) -> MdbxResult<()> {
        self.with_txn_ptr(|txn_ptr| {
            // SAFETY: Caller ensures no other references exist.
            unsafe { ops::drop_db_raw(txn_ptr, db.dbi()) }
        })??;
        self.meta.db_cache.remove_dbi(db.dbi());
        Ok(())
    }
}

impl TxUnsync<RO> {
    /// Closes the database handle.
    ///
    /// # Safety
    ///
    /// This will invalidate data cached in [`Database`] instances, and may
    /// result in bad behavior when using those instances after calling this
    /// function.
    pub unsafe fn close_db(&mut self, dbi: ffi::MDBX_dbi) -> MdbxResult<()> {
        // SAFETY: Caller ensures no other references exist.
        unsafe { ops::close_db_raw(self.meta.env.env_ptr(), dbi) }?;
        self.meta.db_cache.remove_dbi(dbi);
        Ok(())
    }
}

impl<K: TransactionKind> fmt::Debug for TxUnsync<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transaction").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_basic_rw_operations() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        // Write data
        let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
        let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
        txn.put(db, b"key1", b"value1", WriteFlags::empty()).unwrap();
        txn.put(db, b"key2", b"value2", WriteFlags::empty()).unwrap();
        txn.commit().unwrap();

        // Read data
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
    fn test_db_cache() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().set_max_dbs(10).open(dir.path()).unwrap();

        // Create named DBs
        {
            let mut txn = TxUnsync::<RW>::new(env.clone()).unwrap();
            txn.create_db(Some("db1"), DatabaseFlags::empty()).unwrap();
            txn.create_db(Some("db2"), DatabaseFlags::empty()).unwrap();
            txn.commit().unwrap();
        }

        let mut txn = TxUnsync::<RO>::new(env).unwrap();

        let db1_a = txn.open_db(Some("db1")).unwrap();
        let db1_b = txn.open_db(Some("db1")).unwrap();
        let db2 = txn.open_db(Some("db2")).unwrap();

        assert_eq!(db1_a.dbi(), db1_b.dbi());
        assert_ne!(db1_a.dbi(), db2.dbi());
    }

    #[test]
    #[cfg(feature = "read-tx-timeouts")]
    fn test_ro_transaction_no_timeout() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();

        let mut txn = TxUnsync::<RO>::new_no_timeout(env).unwrap();
        let db = txn.open_db(None).unwrap();
        let value: Option<Vec<u8>> = txn.get(db.dbi(), b"missing").unwrap();
        assert!(value.is_none());
    }
}
