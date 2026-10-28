use crate::{
    database::Database,
    environment::Environment,
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    txn_manager::{TxnManagerMessage, TxnPtr},
    Cursor, Error, Stat, TableObject,
};
use ffi::{MDBX_txn_flags_t, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE};
use std::{
    cell::RefCell,
    ffi::{c_uint, c_void},
    fmt::{self, Debug},
    marker::PhantomData,
    mem::size_of,
    ptr, slice,
    sync::mpsc::sync_channel,
    time::Duration,
};

#[cfg(feature = "read-tx-timeouts")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for RO {}
    impl Sealed for RW {}
}

pub trait TransactionKind: private::Sealed + Send + Sync + Debug + 'static {
    #[doc(hidden)]
    const OPEN_FLAGS: MDBX_txn_flags_t;

    /// Convenience flag for distinguishing between read-only and read-write transactions.
    #[doc(hidden)]
    const IS_READ_ONLY: bool;
}

#[derive(Debug)]
#[non_exhaustive]
pub struct RO;

#[derive(Debug)]
#[non_exhaustive]
pub struct RW;

impl TransactionKind for RO {
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_RDONLY;
    const IS_READ_ONLY: bool = true;
}
impl TransactionKind for RW {
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_READWRITE;
    const IS_READ_ONLY: bool = false;
}

/// An MDBX transaction.
///
/// All database operations require a transaction. A transaction can be sent to another thread,
/// but not shared between threads, because MDBX requires a transaction and its cursors to be used
/// by one thread at a time.
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<reth_libmdbx::Transaction<reth_libmdbx::RO>>();
/// ```
pub struct Transaction<K>
where
    K: TransactionKind,
{
    /// The transaction pointer itself.
    txn: *mut ffi::MDBX_txn,
    /// Set by the read transaction monitor in `TxnManager` once the transaction has been open for
    /// too long, or `None` if the transaction isn't monitored. Every following operation fails,
    /// but the monitor doesn't reset the transaction, so it stays valid until dropped.
    #[cfg(feature = "read-tx-timeouts")]
    timed_out: Option<Arc<AtomicBool>>,
    /// Held while a [`Self::txn_execute`] closure runs, to reject reentrant use of the
    /// transaction, for example by a [`TableObject`] decoder that reaches it through a thread
    /// local.
    executing: RefCell<()>,
    /// Whether the transaction has committed.
    committed: bool,
    env: Environment,
    _marker: PhantomData<fn(K)>,
}

impl<K> Transaction<K>
where
    K: TransactionKind,
{
    pub(crate) fn new(env: Environment) -> Result<Self> {
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_txn_begin_ex(
                env.env_ptr(),
                ptr::null_mut(),
                K::OPEN_FLAGS,
                &mut txn,
                ptr::null_mut(),
            ))?;
            Ok(Self::new_from_ptr(env, txn))
        }
    }

    pub(crate) fn new_from_ptr(env: Environment, txn: *mut ffi::MDBX_txn) -> Self {
        Self {
            txn,
            #[cfg(feature = "read-tx-timeouts")]
            timed_out: if K::IS_READ_ONLY {
                env.txn_manager().add_active_read_transaction(txn)
            } else {
                None
            },
            executing: RefCell::new(()),
            committed: false,
            env,
            _marker: PhantomData,
        }
    }

    /// Executes the given closure with the raw transaction pointer.
    ///
    /// Returns [`Error::ReadTransactionTimeout`] instead if the read transaction has timed out.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the transaction.
    ///
    /// # Panics
    ///
    /// Panics if called from inside another `txn_execute` closure on the same transaction.
    #[inline]
    pub fn txn_execute<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        #[cfg(feature = "read-tx-timeouts")]
        if self.timed_out.as_ref().is_some_and(|timed_out| timed_out.load(Ordering::Relaxed)) {
            return Err(Error::ReadTransactionTimeout)
        }

        // Values read by `f` point into MDBX pages, which a reentrant write could modify. The guard
        // is released when `f` returns or unwinds.
        let _guard = self.executing.try_borrow_mut().expect("reentrant use of an MDBX transaction");
        Ok(f(self.txn))
    }

    /// Returns a copy of the raw pointer to the underlying MDBX transaction.
    #[doc(hidden)]
    #[cfg(test)]
    pub const fn txn(&self) -> *mut ffi::MDBX_txn {
        self.txn
    }

    /// Returns a raw pointer to the MDBX environment.
    pub const fn env(&self) -> &Environment {
        &self.env
    }

    /// Returns the transaction id.
    pub fn id(&self) -> Result<u64> {
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
    pub fn get<Key>(&self, dbi: ffi::MDBX_dbi, key: &[u8]) -> Result<Option<Key>>
    where
        Key: TableObject,
    {
        let key_val: ffi::MDBX_val =
            ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

        self.txn_execute(|txn| unsafe {
            match ffi::mdbx_get(txn, dbi, &key_val, &mut data_val) {
                ffi::MDBX_SUCCESS => Key::decode_val::<K>(txn, data_val).map(Some),
                ffi::MDBX_NOTFOUND => Ok(None),
                err_code => Err(Error::from_err_code(err_code)),
            }
        })?
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    pub fn commit(mut self) -> Result<CommitLatency> {
        match self.txn_execute(|txn| {
            if K::IS_READ_ONLY {
                #[cfg(feature = "read-tx-timeouts")]
                self.env().txn_manager().remove_active_read_transaction(txn);

                let mut latency = CommitLatency::new();
                mdbx_result(unsafe { ffi::mdbx_txn_commit_ex(txn, latency.mdbx_commit_latency()) })
                    .map(|v| (v, latency))
            } else {
                let (sender, rx) = sync_channel(0);
                self.env()
                    .txn_manager()
                    .send_message(TxnManagerMessage::Commit { tx: TxnPtr(txn), sender });
                rx.recv().unwrap()
            }
        })? {
            //
            Ok((false, lat)) => {
                self.committed = true;
                Ok(lat)
            }
            Ok((true, _)) => {
                // MDBX_RESULT_TRUE means the transaction was aborted due to prior errors.
                // The transaction is still finished/freed by MDBX, so we must mark it as
                // committed to prevent the Drop impl from trying to abort it again.
                self.committed = true;
                Err(Error::BotchedTransaction)
            }
            Err(e) => Err(e),
        }
    }

    /// Opens a handle to an MDBX database.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// [`EnvironmentBuilder::set_max_dbs()`](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// The returned database handle may be shared among any transaction in the environment.
    ///
    /// The database name may not contain the null character.
    pub fn open_db(&self, name: Option<&str>) -> Result<Database> {
        Database::new(self, name, 0)
    }

    /// Gets the option flags for the given database in the transaction.
    pub fn db_flags(&self, dbi: ffi::MDBX_dbi) -> Result<DatabaseFlags> {
        let mut flags: c_uint = 0;
        unsafe {
            self.txn_execute(|txn| {
                // `mdbx_dbi_flags_ex` requires `status` to be a non-NULL ptr, otherwise it will
                // return an EINVAL and panic below, so we just provide a placeholder variable
                // which we discard immediately.
                let mut _status: c_uint = 0;
                mdbx_result(ffi::mdbx_dbi_flags_ex(txn, dbi, &mut flags, &mut _status))
            })??;
        }

        // The types are not the same on Windows. Great!
        #[cfg_attr(not(windows), expect(clippy::useless_conversion))]
        Ok(DatabaseFlags::from_bits_truncate(flags.try_into().unwrap()))
    }

    /// Retrieves database statistics.
    pub fn db_stat(&self, dbi: ffi::MDBX_dbi) -> Result<Stat> {
        self.db_stat_with_dbi(dbi)
    }

    /// Retrieves database statistics by the given dbi.
    pub fn db_stat_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            self.txn_execute(|txn| {
                mdbx_result(ffi::mdbx_dbi_stat(txn, dbi, stat.mdbx_stat(), size_of::<Stat>()))
            })??;
            Ok(stat)
        }
    }

    /// Open a new cursor on the given database.
    pub fn cursor(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<'_, K>> {
        Cursor::new(self, dbi)
    }

    /// Open a new cursor on the given dbi.
    pub fn cursor_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<'_, K>> {
        Cursor::new(self, dbi)
    }

    /// Disables a timeout for this read transaction.
    #[cfg(feature = "read-tx-timeouts")]
    pub fn disable_timeout(&self) {
        if K::IS_READ_ONLY {
            self.env().txn_manager().disable_read_transaction_timeout(self.txn);
        }
    }
}

impl<K> fmt::Debug for Transaction<K>
where
    K: TransactionKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoTransaction").finish_non_exhaustive()
    }
}

impl<K> Drop for Transaction<K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        if self.committed {
            return
        }

        let txn = self.txn;
        if K::IS_READ_ONLY {
            #[cfg(feature = "read-tx-timeouts")]
            self.env.txn_manager().remove_active_read_transaction(txn);

            // Reset and return the handle to the pool for lock-free reuse.
            // pool.put() calls mdbx_txn_reset internally and falls back to
            // mdbx_txn_abort if the reset fails or the pool is full.
            self.env.ro_txn_pool().push(txn);
        } else {
            let (sender, rx) = sync_channel(0);
            self.env
                .txn_manager()
                .send_message(TxnManagerMessage::Abort { tx: TxnPtr(txn), sender });
            if let Ok(Err(e)) = rx.recv() {
                tracing::error!(target: "libmdbx", %e, "failed to abort transaction in drop");
            }
        }
    }
}

impl Transaction<RW> {
    fn open_db_with_flags(&self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        Database::new(self, name, flags.bits())
    }

    /// Opens a handle to an MDBX database, creating the database if necessary.
    ///
    /// If the database is already created, the given option flags will be added to it.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// [`EnvironmentBuilder::set_max_dbs()`](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// This function will fail with [`Error::BadRslot`] if called by a thread with an open
    /// transaction.
    pub fn create_db(&self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default
    /// behavior is to enter the new key/data pair, replacing any previously
    /// existing key if duplicates are disallowed, or adding a duplicate data
    /// item if duplicates are allowed ([`DatabaseFlags::DUP_SORT`]).
    pub fn put(
        &self,
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
        mdbx_result(self.txn_execute(|txn| unsafe {
            ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits())
        })?)?;

        Ok(())
    }

    /// Returns a buffer which can be used to write a value into the item at the
    /// given key and with the given length. The buffer must be completely
    /// filled by the caller.
    ///
    /// This should not be used on dupsort tables.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned buffer is not used after the transaction is
    /// committed or aborted, or if another value is inserted. To be clear: the second call to
    /// this function is not permitted while the returned slice is reachable.
    #[expect(clippy::mut_from_ref)]
    pub unsafe fn reserve(
        &self,
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
            mdbx_result(self.txn_execute(|txn| {
                ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits() | ffi::MDBX_RESERVE)
            })?)?;
            Ok(slice::from_raw_parts_mut(data_val.iov_base as *mut u8, data_val.iov_len))
        }
    }

    /// Delete items from a database.
    /// This function removes key/data pairs from the database.
    ///
    /// The data parameter is NOT ignored regardless the database does support sorted duplicate data
    /// items or not. If the data parameter is [Some] only the matching data item will be
    /// deleted. Otherwise, if data parameter is [None], any/all value(s) for specified key will
    /// be deleted.
    ///
    /// Returns `true` if the key/value pair was present.
    pub fn del(
        &self,
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
            self.txn_execute(|txn| {
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
    pub fn clear_db(&self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(self.txn_execute(|txn| unsafe { ffi::mdbx_drop(txn, dbi, false) })?)?;

        Ok(())
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing
    /// to the same dbi BEFORE calling this function.
    pub unsafe fn drop_db(&self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(self.txn_execute(|txn| unsafe { ffi::mdbx_drop(txn, dbi, true) })?)?;

        Ok(())
    }
}

impl Transaction<RO> {
    /// Closes the database handle.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi
    /// BEFORE calling this function.
    pub unsafe fn close_db(&self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(unsafe { ffi::mdbx_dbi_close(self.env().env_ptr(), dbi) })?;

        Ok(())
    }
}

impl Transaction<RW> {
    /// Begins a new nested transaction inside of this transaction.
    pub fn begin_nested_txn(&mut self) -> Result<Self> {
        if self.env.is_write_map() {
            return Err(Error::NestedTransactionsUnsupportedWithWriteMap)
        }
        self.txn_execute(|txn| {
            let (tx, rx) = sync_channel(0);
            self.env().txn_manager().send_message(TxnManagerMessage::Begin {
                parent: TxnPtr(txn),
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
#[derive(Debug)]
#[repr(transparent)]
pub struct CommitLatency(ffi::MDBX_commit_latency);

impl CommitLatency {
    /// Create a new `CommitLatency` with zero'd inner struct `ffi::MDBX_commit_latency`.
    pub(crate) const fn new() -> Self {
        unsafe { Self(std::mem::zeroed()) }
    }

    /// Returns a mut pointer to `ffi::MDBX_commit_latency`.
    pub(crate) const fn mdbx_commit_latency(&mut self) -> *mut ffi::MDBX_commit_latency {
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

// SAFETY: `Transaction` owns its pointer and is not `Sync`, and its cursors borrow it, so only one
// thread uses it at a time. `MDBX_NOSTICKYTHREADS` lets that thread differ from the one that
// started the transaction. Write transactions are still started, committed, and aborted on the
// `TxnManager` thread.
unsafe impl<K> Send for Transaction<K> where K: TransactionKind {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    const fn assert_send<T: Send>() {}

    #[expect(dead_code)]
    const fn test_txn_send() {
        assert_send::<Transaction<RO>>();
        assert_send::<Transaction<RW>>();
    }

    #[test]
    #[should_panic(expected = "reentrant use of an MDBX transaction")]
    fn reentrant_txn_execute_panics() {
        let dir = tempfile::tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        let txn = env.begin_rw_txn().unwrap();
        let _ = txn.txn_execute(|_| txn.txn_execute(|_| ()));
    }

    #[test]
    fn txn_execute_after_panic() {
        let dir = tempfile::tempdir().unwrap();
        let env = Environment::builder().open(dir.path()).unwrap();
        let txn = env.begin_ro_txn().unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = txn.txn_execute(|_| panic!("decoder failed"));
        }));
        assert!(result.is_err());
        txn.id().unwrap();
    }
}
