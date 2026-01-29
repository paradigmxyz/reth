#[cfg(debug_assertions)]
use crate::tx::assertions;
use crate::{
    error::{mdbx_result, MdbxResult},
    flags::{DatabaseFlags, WriteFlags},
    sys::txn_manager::{RawTxPtr, TxnManagerMessage},
    tx::{ops, CachedDb, PtrSync, PtrSyncInner, SharedCache, TxPtrAccess},
    Cursor, Database, Environment, MdbxError, ReadResult, Stat, TableObject, TransactionKind, RO,
    RW,
};
use smallvec::SmallVec;
use std::{
    ffi::CStr,
    fmt, ptr,
    sync::{mpsc::sync_channel, Arc},
    time::Duration,
};

/// An MDBX transaction.
///
/// All database operations require a transaction.
pub struct TxSync<K>
where
    K: TransactionKind,
{
    inner: Arc<SyncInner<K>>,
}

impl<K> TxSync<K>
where
    K: TransactionKind,
{
    pub(crate) fn new_from_ptr(env: Environment, txn_ptr: *mut ffi::MDBX_txn) -> Self {
        let txn = PtrSync::<K>::new(env, txn_ptr);

        let inner = SyncInner { ptr: txn, db_cache: SharedCache::default() };

        Self { inner: Arc::new(inner) }
    }

    /// Executes the given closure once the lock on the transaction is acquired.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the transaction.
    #[inline]
    pub fn txn_execute<F, T>(&self, f: F) -> MdbxResult<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.inner.txn_execute(f)
    }

    /// Returns a raw pointer to the MDBX environment.
    pub fn env(&self) -> &Environment {
        self.inner.env()
    }

    /// Returns the tracing span for this transaction.
    ///
    /// Users can enter this span to associate operations with the transaction:
    /// ```ignore
    /// let _guard = txn.span().enter();
    /// // operations here are within the transaction span
    /// ```
    pub fn span(&self) -> &tracing::Span {
        self.inner.span()
    }

    /// Returns the transaction id.
    pub fn id(&self) -> MdbxResult<u64> {
        self.txn_execute(|txn| unsafe { ffi::mdbx_txn_id(txn) })
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the
    /// database. If the database supports duplicate keys
    /// ([`DatabaseFlags::DUP_SORT`]) then the first data item for the key will be
    /// returned. Retrieval of other items requires the use of
    /// [Cursor]. If the item is not in the database, then
    /// [None] will be returned.
    pub fn get<'a, Key>(&'a self, dbi: ffi::MDBX_dbi, key: &[u8]) -> ReadResult<Option<Key>>
    where
        Key: TableObject<'a>,
    {
        self.txn_execute(|txn_ptr| {
            // SAFETY:
            // txn is a valid transaction pointer from txn_execute.
            // The decoded Cow is valid as long as the data is not dirty, and
            // the tx is alive.
            // The lifetime 'tx statically guarantees that the Cow cannot
            // outlive the transaction.
            // `decode_val` checks for dirty writes and copies data if needed.
            unsafe {
                let data_val = ops::get_raw(txn_ptr, dbi, key)?;
                data_val.map(|val| Key::decode_val::<K>(txn_ptr, val)).transpose()
            }
        })?
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    pub fn commit(self) -> MdbxResult<CommitLatency> {
        let _guard = self.inner.span().enter();

        let (was_aborted, lat) = self.txn_execute(|txn| {
            if K::IS_READ_ONLY {
                #[cfg(feature = "read-tx-timeouts")]
                self.env().txn_manager().remove_active_read_transaction(txn);

                let mut latency = CommitLatency::new();
                mdbx_result(unsafe { ffi::mdbx_txn_commit_ex(txn, latency.mdb_commit_latency()) })
                    .map(|v| (v, latency))
            } else {
                let (sender, rx) = sync_channel(0);
                self.env()
                    .txn_manager()
                    .send_message(TxnManagerMessage::Commit { tx: RawTxPtr(txn), sender });
                rx.recv().unwrap()
            }
        })??;

        self.inner.ptr.mark_committed();

        if was_aborted {
            tracing::warn!(target: "libmdbx", "botched");
            return Err(MdbxError::BotchedTransaction);
        }

        tracing::debug!(
            target: "libmdbx",
            latency_whole_ms = lat.whole().as_millis() as u64,
            "committed"
        );
        Ok(lat)
    }

    /// Opens a handle to an MDBX database, and cache the handle for re-use.
    ///
    /// If `name` is `None`, then the returned handle will be for the default
    /// database.
    ///
    /// If `name` is not `None`, then the returned handle will be for a named
    /// database. In this case the environment must be configured to allow
    /// named databases through
    /// [`EnvironmentBuilder::set_max_dbs()`](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// The returned database handle MAY be shared among any transaction in the
    /// environment. However, if the tx is RW and the DB is created within the
    /// tx, the DB will not be visible to other transactions until the tx is
    /// committed.
    ///
    /// The database name MAY NOT contain the null character.
    pub fn open_db(&self, name: Option<&str>) -> MdbxResult<Database> {
        let name_hash = CachedDb::hash_name(name);

        if let Some(db) = self.inner.db_cache.read_db(name_hash) {
            return Ok(db);
        }

        self.open_and_cache_with_flags(name, DatabaseFlags::empty()).map(Into::into)
    }

    /// Open a DB handle without checking or writing to the cache.
    ///
    /// This may be useful when the transaction intends to open many (>20)
    /// tables, as cache performance will degrade slightly with size.
    pub fn open_db_no_cache(&self, name: Option<&str>) -> MdbxResult<Database> {
        self.open_db_with_flags(name, DatabaseFlags::empty()).map(Into::into)
    }

    /// Raw open (don't check cache) with flags. Write to cache after opening.
    fn open_and_cache_with_flags(
        &self,
        name: Option<&str>,
        flags: DatabaseFlags,
    ) -> Result<CachedDb, MdbxError> {
        // Slow path: open via FFI and cache
        let db = self.open_db_with_flags(name, flags)?;

        // Double-check pattern to avoid duplicate entries
        self.inner.db_cache.write_db(db);

        Ok(db)
    }

    /// Raw open (don't check cache) with flags.
    ///
    /// Return the name hash along with the database.
    fn open_db_with_flags(&self, name: Option<&str>, flags: DatabaseFlags) -> MdbxResult<CachedDb> {
        let mut c_name_buf = SmallVec::<[u8; 32]>::new();
        let c_name = name.map(|n| {
            c_name_buf.extend_from_slice(n.as_bytes());
            c_name_buf.push(0);
            CStr::from_bytes_with_nul(&c_name_buf).unwrap()
        });
        let name_ptr = c_name.as_ref().map_or(ptr::null(), |s| s.as_ptr());

        let (dbi, db_flags) = self.txn_execute(|txn_ptr| {
            // SAFETY: txn_ptr is valid from txn_execute, name_ptr is valid or null.
            unsafe { ops::open_db_raw(txn_ptr, name_ptr, flags) }
        })??;

        Ok(CachedDb::new(name, Database::new(dbi, db_flags)))
    }

    /// Gets the option flags for the given database in the transaction.
    pub fn db_flags(&self, dbi: ffi::MDBX_dbi) -> MdbxResult<DatabaseFlags> {
        self.txn_execute(|txn| {
            // SAFETY: txn is a valid transaction pointer from txn_execute.
            unsafe { ops::db_flags_raw(txn, dbi) }
        })?
    }

    /// Retrieves database statistics.
    pub fn db_stat(&self, dbi: ffi::MDBX_dbi) -> MdbxResult<Stat> {
        self.db_stat_with_dbi(dbi)
    }

    /// Retrieves database statistics by the given dbi.
    pub fn db_stat_with_dbi(&self, dbi: ffi::MDBX_dbi) -> MdbxResult<Stat> {
        self.txn_execute(|txn| {
            // SAFETY: txn is a valid transaction pointer from txn_execute.
            unsafe { ops::db_stat_raw(txn, dbi) }
        })?
    }

    /// Open a new cursor on the given database.
    pub fn cursor(&self, db: Database) -> MdbxResult<Cursor<'_, K, PtrSyncInner<K>>> {
        Cursor::new(&self.inner.ptr, db)
    }

    /// Open a new cursor on the given dbi.
    #[deprecated(since = "0.2.0", note = "use `cursor(&Database)` instead")]
    pub fn cursor_with_dbi(
        &self,
        dbi: ffi::MDBX_dbi,
    ) -> MdbxResult<Cursor<'_, K, PtrSyncInner<K>>> {
        let db = Database::new(dbi, DatabaseFlags::empty());
        Cursor::new(&self.inner.ptr, db)
    }

    /// Disables a timeout for this read transaction.
    #[cfg(feature = "read-tx-timeouts")]
    pub fn disable_timeout(&self) {
        if K::IS_READ_ONLY {
            // SAFETY: Not performing any operation on the txn, just updating
            // internal state.
            self.env()
                .txn_manager()
                .remove_active_read_transaction(unsafe { self.inner.ptr.txn_ptr() });
        }
    }
}

impl<K> Clone for TxSync<K>
where
    K: TransactionKind,
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<K> fmt::Debug for TxSync<K>
where
    K: TransactionKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoTransaction").finish_non_exhaustive()
    }
}

/// Internals of a transaction.
struct SyncInner<K>
where
    K: TransactionKind,
{
    /// The transaction pointer itself.
    ptr: PtrSync<K>,

    /// Cache of opened database handles.
    db_cache: SharedCache,
}

impl<K> SyncInner<K>
where
    K: TransactionKind,
{
    fn env(&self) -> &Environment {
        self.ptr.env()
    }

    fn span(&self) -> &tracing::Span {
        self.ptr.span()
    }

    #[inline]
    fn txn_execute<F, T>(&self, f: F) -> MdbxResult<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.ptr.txn_execute_fail_on_timeout(f)
    }
}

impl<K: TransactionKind> Drop for SyncInner<K> {
    fn drop(&mut self) {
        #[cfg(feature = "read-tx-timeouts")]
        if K::IS_READ_ONLY {
            // Remove from active read transactions before dropping PtrSync.
            // This breaks the circular Arc reference: the map holds a PtrSync
            // clone, so we must remove it before the final reference is dropped.
            //
            // SAFETY: Not performing any MDBX operation, just updating internal
            // tracking state.
            unsafe {
                self.ptr.env().txn_manager().remove_active_read_transaction(self.ptr.txn_ptr());
            }
        }
    }
}

impl TxSync<RW> {
    /// Opens a handle to an MDBX database, creating the database if necessary.
    ///
    /// If the database is already created, the given option flags will be
    /// added to it.
    ///
    /// If `name` is [None], then the returned handle will be for the default
    /// database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named
    /// database. In this case the environment must be configured to allow
    /// named databases through [`EnvironmentBuilder::set_max_dbs()`].
    ///
    /// This function will fail with [`MdbxError::BadRslot`] if called by a
    /// thread with an open transaction.
    ///
    /// [`EnvironmentBuilder::set_max_dbs()`]: crate::EnvironmentBuilder::set_max_dbs
    pub fn create_db(&self, name: Option<&str>, flags: DatabaseFlags) -> MdbxResult<Database> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE).map(Into::into)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default
    /// behavior is to enter the new key/data pair, replacing any previously
    /// existing key if duplicates are disallowed, or adding a duplicate data
    /// item if duplicates are allowed ([`DatabaseFlags::DUP_SORT`]).
    pub fn put(
        &self,
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

        self.txn_execute(|txn| {
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::put_raw(txn, db.dbi(), key, data, flags) }
        })?
    }

    /// Appends a key/data pair to the end of the database.
    ///
    /// The key must be greater than all existing keys (or less than, for
    /// [`DatabaseFlags::REVERSE_KEY`] tables). This is more efficient than
    /// [`TxSync::put`] when adding data in sorted order.
    ///
    /// In debug builds, this method asserts that the key ordering constraint is
    /// satisfied.
    pub fn append(
        &self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
    ) -> MdbxResult<()> {
        let key = key.as_ref();
        let data = data.as_ref();

        self.txn_execute(|txn| {
            #[cfg(debug_assertions)]
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe {
                ops::debug_assert_append(txn, db.dbi(), db.flags(), key, data);
            }

            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::put_raw(txn, db.dbi(), key, data, WriteFlags::APPEND) }
        })?
    }

    /// Appends duplicate data for [`DatabaseFlags::DUP_SORT`] databases.
    ///
    /// The data must be greater than all existing data for this key (or less
    /// than, for [`DatabaseFlags::REVERSE_DUP`] tables). This is more efficient
    /// than [`TxSync::put`] when adding duplicates in sorted order.
    ///
    /// Returns [`MdbxError::RequiresDupSort`] if the database does not have the
    /// [`DatabaseFlags::DUP_SORT`] flag set.
    ///
    /// In debug builds, this method asserts that the data ordering constraint
    /// is satisfied.
    pub fn append_dup(
        &self,
        db: Database,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
    ) -> MdbxResult<()> {
        if !db.flags().contains(DatabaseFlags::DUP_SORT) {
            return Err(MdbxError::RequiresDupSort);
        }
        let key = key.as_ref();
        let data = data.as_ref();

        self.txn_execute(|txn| {
            #[cfg(debug_assertions)]
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe {
                ops::debug_assert_append_dup(txn, db.dbi(), db.flags(), key, data);
            }

            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::put_raw(txn, db.dbi(), key, data, WriteFlags::APPEND_DUP) }
        })?
    }

    /// Returns a buffer which can be used to write a value into the item at the
    /// given key and with the given length. The buffer must be completely
    /// filled by the caller.
    ///
    /// This should not be used on dupsort tables.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned buffer is not used after the
    /// transaction is committed or aborted, or if another value is inserted.
    /// To be clear: the second call to this function is not permitted while
    /// the returned slice is reachable.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn reserve(
        &self,
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

        let ptr = self.txn_execute(|txn| {
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::reserve_raw(txn, db.dbi(), key, len, flags) }
        })??;
        // SAFETY: ptr is valid from reserve_raw, len matches.
        Ok(unsafe { ops::slice_from_reserved(ptr, len) })
    }

    /// Reserves space for a value of the given length at the given key, and
    /// calls the given closure with a mutable slice to write into.
    ///
    /// This is a safe wrapper around [`TxSync::reserve`].
    pub fn with_reservation(
        &self,
        db: Database,
        key: impl AsRef<[u8]>,
        len: usize,
        flags: WriteFlags,
        f: impl FnOnce(&mut [u8]),
    ) -> MdbxResult<()> {
        let buf = unsafe { self.reserve(db, key, len, flags)? };
        f(buf);
        Ok(())
    }

    /// Delete items from a database.
    /// This function removes key/data pairs from the database.
    ///
    /// The data parameter is NOT ignored regardless the database does support
    /// sorted duplicate data items or not. If the data parameter is [Some]
    /// only the matching data item will be deleted. Otherwise, if data
    /// parameter is [None], any/all value(s) for specified key will
    /// be deleted.
    ///
    /// Returns `true` if the key/value pair was present.
    pub fn del(
        &self,
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

        self.txn_execute(|txn| {
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::del_raw(txn, db.dbi(), key, data) }
        })?
    }

    /// Empties the given database. All items will be removed.
    pub fn clear_db(&self, db: Database) -> MdbxResult<()> {
        self.txn_execute(|txn| {
            // SAFETY: txn is a valid RW transaction pointer from txn_execute.
            unsafe { ops::clear_db_raw(txn, db.dbi()) }
        })?
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing
    /// to the same dbi BEFORE calling this function.
    pub unsafe fn drop_db(&self, db: Database) -> MdbxResult<()> {
        self.txn_execute(|txn| {
            // SAFETY: txn is a valid RW transaction pointer, caller ensures
            // no other references to dbi exist.
            unsafe { ops::drop_db_raw(txn, db.dbi()) }
        })??;

        self.inner.db_cache.remove_dbi(db.dbi());

        Ok(())
    }
}

impl TxSync<RO> {
    pub(crate) fn new(env: Environment) -> MdbxResult<Self> {
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
        let this = Self::new_from_ptr(env, txn);

        #[cfg(feature = "read-tx-timeouts")]
        this.env().txn_manager().add_active_read_transaction(txn, this.inner.ptr.clone());

        Ok(this)
    }

    /// Closes the database handle.
    ///
    /// # Safety
    ///
    /// This will invalidate data cached in [`Database`] instances, and may
    /// result in bad behavior when using those instances after calling this
    /// function.
    pub unsafe fn close_db(&self, dbi: ffi::MDBX_dbi) -> MdbxResult<()> {
        // SAFETY: Caller ensures the database is not in use.
        unsafe { ops::close_db_raw(self.env().env_ptr(), dbi) }?;

        self.inner.db_cache.remove_dbi(dbi);

        Ok(())
    }
}

impl TxSync<RW> {
    /// Begins a new nested transaction inside of this transaction.
    pub fn begin_nested_txn(&mut self) -> MdbxResult<Self> {
        if self.inner.ptr.env().is_write_map() {
            return Err(MdbxError::NestedTransactionsUnsupportedWithWriteMap);
        }
        self.txn_execute(|txn| {
            let (tx, rx) = sync_channel(0);
            self.env().txn_manager().send_message(TxnManagerMessage::Begin {
                parent: RawTxPtr(txn),
                flags: RW::OPEN_FLAGS,
                sender: tx,
            });

            rx.recv().unwrap().map(|ptr| Self::new_from_ptr(self.env().clone(), ptr.0))
        })?
    }
}

/// Commit latencies info.
///
/// Contains information about latency of commit stages.
/// Inner struct stores this info in 1/65536 of seconds units.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct CommitLatency(ffi::MDBX_commit_latency);

impl CommitLatency {
    /// Create a new `CommitLatency` with zero'd inner struct `ffi::MDBX_commit_latency`.
    pub(crate) const fn new() -> Self {
        unsafe { Self(std::mem::zeroed()) }
    }

    /// Returns a mut pointer to `ffi::MDBX_commit_latency`.
    pub(crate) const fn mdb_commit_latency(&mut self) -> *mut ffi::MDBX_commit_latency {
        &mut self.0
    }
}

impl CommitLatency {
    /// Duration of preparation (commit child transactions, update
    /// sub-databases records and cursors destroying).
    #[inline]
    pub const fn preparation(&self) -> Duration {
        Self::time_to_duration(self.0.preparation)
    }

    /// Duration of GC update by wall clock.
    #[inline]
    pub const fn gc_wallclock(&self) -> Duration {
        Self::time_to_duration(self.0.gc_wallclock)
    }

    /// Duration of internal audit if enabled.
    #[inline]
    pub const fn audit(&self) -> Duration {
        Self::time_to_duration(self.0.audit)
    }

    /// Duration of writing dirty/modified data pages to a filesystem,
    /// i.e. the summary duration of a `write()` syscalls during commit.
    #[inline]
    pub const fn write(&self) -> Duration {
        Self::time_to_duration(self.0.write)
    }

    /// Duration of syncing written data to the disk/storage, i.e.
    /// the duration of a `fdatasync()` or a `msync()` syscall during commit.
    #[inline]
    pub const fn sync(&self) -> Duration {
        Self::time_to_duration(self.0.sync)
    }

    /// Duration of transaction ending (releasing resources).
    #[inline]
    pub const fn ending(&self) -> Duration {
        Self::time_to_duration(self.0.ending)
    }

    /// The total duration of a commit.
    #[inline]
    pub const fn whole(&self) -> Duration {
        Self::time_to_duration(self.0.whole)
    }

    /// User-mode CPU time spent on GC update.
    #[inline]
    pub const fn gc_cputime(&self) -> Duration {
        Self::time_to_duration(self.0.gc_cputime)
    }

    #[inline]
    const fn time_to_duration(time: u32) -> Duration {
        Duration::from_nanos(time as u64 * (1_000_000_000 / 65_536))
    }
}

// SAFETY: Access to the transaction is synchronized by the lock.
unsafe impl<K: TransactionKind> Send for PtrSync<K> {}

// SAFETY: Access to the transaction is synchronized by the lock.
unsafe impl<K: TransactionKind> Sync for PtrSync<K> {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const fn assert_send_sync<T: Send + Sync>() {}

    #[expect(dead_code)]
    const fn test_txn_send_sync() {
        assert_send_sync::<TxSync<RO>>();
        assert_send_sync::<TxSync<RW>>();
    }

    #[test]
    fn test_db_cache_returns_same_db() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        let txn = env.begin_ro_txn().unwrap();

        let db1 = txn.open_db(None).unwrap();
        let db2 = txn.open_db(None).unwrap();

        assert_eq!(db1.dbi(), db2.dbi());
        assert_eq!(db1.flags(), db2.flags());
    }

    #[test]
    fn test_db_cache_no_cache_still_works() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        let txn = env.begin_ro_txn().unwrap();

        let db1 = txn.open_db_no_cache(None).unwrap();
        let db2 = txn.open_db_no_cache(None).unwrap();

        // Same DBI should be returned by MDBX
        assert_eq!(db1.dbi(), db2.dbi());
    }

    #[test]
    fn test_db_cache_cached_matches_uncached() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        let txn = env.begin_ro_txn().unwrap();

        let cached = txn.open_db(None).unwrap();
        let uncached = txn.open_db_no_cache(None).unwrap();

        assert_eq!(cached.dbi(), uncached.dbi());
        assert_eq!(cached.flags(), uncached.flags());
    }

    #[test]
    fn test_db_cache_multiple_named_dbs() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().set_max_dbs(10).open(dir.path()).unwrap();

        // Create named DBs
        {
            let txn = env.begin_rw_txn().unwrap();
            txn.create_db(Some("db1"), DatabaseFlags::empty()).unwrap();
            txn.create_db(Some("db2"), DatabaseFlags::empty()).unwrap();
            txn.commit().unwrap();
        }

        let txn = env.begin_ro_txn().unwrap();

        let db1_a = txn.open_db(Some("db1")).unwrap();
        let db2_a = txn.open_db(Some("db2")).unwrap();
        let db1_b = txn.open_db(Some("db1")).unwrap();
        let db2_b = txn.open_db(Some("db2")).unwrap();

        // Same named DB returns same handle
        assert_eq!(db1_a.dbi(), db1_b.dbi());
        assert_eq!(db2_a.dbi(), db2_b.dbi());

        // Different DBs have different handles
        assert_ne!(db1_a.dbi(), db2_a.dbi());
    }

    #[test]
    fn test_db_cache_flags_preserved() {
        let dir = tempdir().unwrap();
        let env = Environment::builder().set_max_dbs(10).open(dir.path()).unwrap();

        // Create DB with specific flags
        {
            let txn = env.begin_rw_txn().unwrap();
            txn.create_db(Some("dupsort"), DatabaseFlags::DUP_SORT).unwrap();
            txn.commit().unwrap();
        }

        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(Some("dupsort")).unwrap();

        assert!(db.flags().contains(DatabaseFlags::DUP_SORT));

        // Second open should have same flags from cache
        let db2 = txn.open_db(Some("dupsort")).unwrap();
        assert!(db2.flags().contains(DatabaseFlags::DUP_SORT));
    }
}
