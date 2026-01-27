use crate::{
    database::Database,
    environment::Environment,
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    mdbx_try_optional,
    txn_manager::{TxnManagerMessage, TxnPtr},
    Cursor, Error, Stat, TableObject,
};
use ffi::{
    MDBX_cursor_op, MDBX_txn_flags_t, MDBX_FIRST, MDBX_GET_BOTH_RANGE, MDBX_GET_CURRENT, MDBX_LAST,
    MDBX_LAST_DUP, MDBX_NEXT, MDBX_NEXT_DUP, MDBX_NEXT_NODUP, MDBX_PREV, MDBX_PREV_DUP, MDBX_SET,
    MDBX_SET_KEY, MDBX_SET_RANGE, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE,
};
use parking_lot::{Mutex, MutexGuard};
use std::{
    ffi::{c_uint, c_void},
    fmt::{self, Debug},
    mem::size_of,
    ptr, slice,
    sync::{atomic::AtomicBool, mpsc::sync_channel, Arc},
    time::Duration,
};

#[cfg(feature = "read-tx-timeouts")]
use ffi::mdbx_txn_renew;

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
/// All database operations require a transaction.
pub struct Transaction<K>
where
    K: TransactionKind,
{
    inner: Arc<TransactionInner<K>>,
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

    pub(crate) fn new_from_ptr(env: Environment, txn_ptr: *mut ffi::MDBX_txn) -> Self {
        let txn = TransactionPtr::new(txn_ptr);

        #[cfg(feature = "read-tx-timeouts")]
        if K::IS_READ_ONLY {
            env.txn_manager().add_active_read_transaction(txn_ptr, txn.clone())
        }

        let inner = TransactionInner {
            txn,
            committed: AtomicBool::new(false),
            env,
            _marker: Default::default(),
        };

        Self { inner: Arc::new(inner) }
    }

    /// Executes the given closure once the lock on the transaction is acquired.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the transaction.
    #[inline]
    pub fn txn_execute<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.inner.txn_execute(f)
    }

    /// Executes the given closure once the lock on the transaction is acquired. If the transaction
    /// is timed out, it will be renewed first.
    ///
    /// Returns the result of the closure or an error if the transaction renewal fails.
    #[inline]
    pub(crate) fn txn_execute_renew_on_timeout<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.inner.txn_execute_renew_on_timeout(f)
    }

    /// Returns a copy of the raw pointer to the underlying MDBX transaction.
    #[doc(hidden)]
    #[cfg(test)]
    pub fn txn(&self) -> *mut ffi::MDBX_txn {
        self.inner.txn.txn
    }

    /// Returns a raw pointer to the MDBX environment.
    pub fn env(&self) -> &Environment {
        &self.inner.env
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
    pub fn commit(self) -> Result<CommitLatency> {
        match self.txn_execute(|txn| {
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
                    .send_message(TxnManagerMessage::Commit { tx: TxnPtr(txn), sender });
                rx.recv().unwrap()
            }
        })? {
            //
            Ok((false, lat)) => {
                self.inner.set_committed();
                Ok(lat)
            }
            Ok((true, _)) => {
                // MDBX_RESULT_TRUE means the transaction was aborted due to prior errors.
                // The transaction is still finished/freed by MDBX, so we must mark it as
                // committed to prevent the Drop impl from trying to abort it again.
                self.inner.set_committed();
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
        #[cfg_attr(not(windows), allow(clippy::useless_conversion))]
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
                mdbx_result(ffi::mdbx_dbi_stat(txn, dbi, stat.mdb_stat(), size_of::<Stat>()))
            })??;
            Ok(stat)
        }
    }

    /// Open a new cursor on the given database.
    pub fn cursor(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<K>> {
        Cursor::new(self.clone(), dbi)
    }

    /// Open a new cursor on the given dbi.
    pub fn cursor_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<K>> {
        Cursor::new(self.clone(), dbi)
    }

    /// Disables a timeout for this read transaction.
    #[cfg(feature = "read-tx-timeouts")]
    pub fn disable_timeout(&self) {
        if K::IS_READ_ONLY {
            self.env().txn_manager().remove_active_read_transaction(self.inner.txn.txn);
        }
    }
}

impl<K> Clone for Transaction<K>
where
    K: TransactionKind,
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
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

/// Internals of a transaction.
struct TransactionInner<K>
where
    K: TransactionKind,
{
    /// The transaction pointer itself.
    txn: TransactionPtr,
    /// Whether the transaction has committed.
    committed: AtomicBool,
    env: Environment,
    _marker: std::marker::PhantomData<fn(K)>,
}

impl<K> TransactionInner<K>
where
    K: TransactionKind,
{
    /// Marks the transaction as committed.
    fn set_committed(&self) {
        self.committed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn has_committed(&self) -> bool {
        self.committed.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[inline]
    fn txn_execute<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.txn.txn_execute_fail_on_timeout(f)
    }

    #[inline]
    fn txn_execute_renew_on_timeout<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.txn.txn_execute_renew_on_timeout(f)
    }
}

impl<K> Drop for TransactionInner<K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        // To be able to abort a timed out transaction, we need to renew it first.
        // Hence the usage of `txn_execute_renew_on_timeout` here.
        //
        // We intentionally ignore errors here because Drop should never panic.
        // MDBX can return errors (e.g., MDBX_PANIC) during abort if the environment
        // is in a fatal state, but panicking in Drop can cause double-panics during
        // unwinding which terminates the process.
        let _ = self.txn.txn_execute_renew_on_timeout(|txn| {
            if !self.has_committed() {
                if K::IS_READ_ONLY {
                    #[cfg(feature = "read-tx-timeouts")]
                    self.env.txn_manager().remove_active_read_transaction(txn);

                    unsafe {
                        ffi::mdbx_txn_abort(txn);
                    }
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
        });
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
    #[allow(clippy::mut_from_ref)]
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
        if self.inner.env.is_write_map() {
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

/// A shareable pointer to an MDBX transaction.
#[derive(Debug, Clone)]
pub(crate) struct TransactionPtr {
    txn: *mut ffi::MDBX_txn,
    #[cfg(feature = "read-tx-timeouts")]
    timed_out: Arc<AtomicBool>,
    lock: Arc<Mutex<()>>,
}

impl TransactionPtr {
    fn new(txn: *mut ffi::MDBX_txn) -> Self {
        Self {
            txn,
            #[cfg(feature = "read-tx-timeouts")]
            timed_out: Arc::new(AtomicBool::new(false)),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Returns `true` if the transaction is timed out.
    ///
    /// When transaction is timed out via `TxnManager`, it's actually reset using
    /// `mdbx_txn_reset`. It makes the transaction unusable (MDBX fails on any usages of such
    /// transactions).
    ///
    /// Importantly, we can't rely on `MDBX_TXN_FINISHED` flag to check if the transaction is timed
    /// out using `mdbx_txn_reset`, because MDBX uses it in other cases too.
    #[cfg(feature = "read-tx-timeouts")]
    fn is_timed_out(&self) -> bool {
        self.timed_out.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(feature = "read-tx-timeouts")]
    pub(crate) fn set_timed_out(&self) {
        self.timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Acquires the inner transaction lock to guarantee exclusive access to the transaction
    /// pointer.
    fn lock(&self) -> MutexGuard<'_, ()> {
        if let Some(lock) = self.lock.try_lock() {
            lock
        } else {
            tracing::trace!(
                target: "libmdbx",
                txn = %self.txn as usize,
                backtrace = %std::backtrace::Backtrace::capture(),
                "Transaction lock is already acquired, blocking...
                To display the full backtrace, run with `RUST_BACKTRACE=full` env variable."
            );
            self.lock.lock()
        }
    }

    /// Executes the given closure once the lock on the transaction is acquired.
    ///
    /// Returns the result of the closure or an error if the transaction is timed out.
    #[inline]
    pub(crate) fn txn_execute_fail_on_timeout<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        let _lck = self.lock();

        // No race condition with the `TxnManager` timing out the transaction is possible here,
        // because we're taking a lock for any actions on the transaction pointer, including a call
        // to the `mdbx_txn_reset`.
        #[cfg(feature = "read-tx-timeouts")]
        if self.is_timed_out() {
            return Err(Error::ReadTransactionTimeout)
        }

        Ok((f)(self.txn))
    }

    /// Executes the given closure once the lock on the transaction is acquired. If the transaction
    /// is timed out, it will be renewed first.
    ///
    /// Returns the result of the closure or an error if the transaction renewal fails.
    #[inline]
    pub(crate) fn txn_execute_renew_on_timeout<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        let _lck = self.lock();

        // To be able to do any operations on the transaction, we need to renew it first.
        #[cfg(feature = "read-tx-timeouts")]
        if self.is_timed_out() {
            mdbx_result(unsafe { mdbx_txn_renew(self.txn) })?;
        }

        Ok((f)(self.txn))
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
unsafe impl Send for TransactionPtr {}

// SAFETY: Access to the transaction is synchronized by the lock.
unsafe impl Sync for TransactionPtr {}

/// Converts an optional byte slice to an MDBX value.
const unsafe fn slice_to_val(slice: Option<&[u8]>) -> ffi::MDBX_val {
    match slice {
        Some(slice) => {
            ffi::MDBX_val { iov_len: slice.len(), iov_base: slice.as_ptr() as *mut c_void }
        }
        None => ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() },
    }
}

/// An unsynchronized MDBX transaction.
///
/// This is a faster variant of [`Transaction`] that avoids `Arc` and `Mutex` overhead.
/// It is **not** `Sync` and requires `&mut self` for operations.
/// Use this when you need single-threaded access without cloning the transaction.
///
/// Note: Unlike [`Transaction`], unsync transactions are not tracked by the read transaction
/// timeout manager. This means they won't be automatically aborted after a timeout.
pub struct TransactionUnsync<K>
where
    K: TransactionKind,
{
    /// The transaction pointer itself.
    txn: *mut ffi::MDBX_txn,
    /// Whether the transaction has committed.
    committed: bool,
    /// The environment.
    env: Environment,
    _marker: std::marker::PhantomData<fn(K)>,
}

impl<K> TransactionUnsync<K>
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
        }

        Ok(Self { txn, committed: false, env, _marker: Default::default() })
    }

    /// Returns the transaction ID.
    pub fn id(&self) -> u64 {
        unsafe { ffi::mdbx_txn_id(self.txn) }
    }

    /// Returns the environment.
    pub const fn env(&self) -> &Environment {
        &self.env
    }

    /// Gets an item from a database.
    pub fn get<Key>(&self, dbi: ffi::MDBX_dbi, key: &[u8]) -> Result<Option<Key>>
    where
        Key: TableObject,
    {
        let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

        unsafe {
            match ffi::mdbx_get(self.txn, dbi, &key_val, &mut data_val) {
                ffi::MDBX_SUCCESS => Key::decode_val::<K>(self.txn, data_val).map(Some),
                ffi::MDBX_NOTFOUND => Ok(None),
                err_code => Err(Error::from_err_code(err_code)),
            }
        }
    }

    /// Commits the transaction.
    pub fn commit(mut self) -> Result<CommitLatency> {
        self.committed = true;

        if K::IS_READ_ONLY {
            let mut latency = CommitLatency::new();
            mdbx_result(unsafe {
                ffi::mdbx_txn_commit_ex(self.txn, latency.mdb_commit_latency())
            })?;
            Ok(latency)
        } else {
            let (sender, rx) = sync_channel(0);
            self.env
                .txn_manager()
                .send_message(TxnManagerMessage::Commit { tx: TxnPtr(self.txn), sender });
            match rx.recv().unwrap() {
                Ok((false, lat)) => Ok(lat),
                Ok((true, _)) => Err(Error::BotchedTransaction),
                Err(e) => Err(e),
            }
        }
    }

    /// Opens a handle to an MDBX database.
    pub fn open_db(&self, name: Option<&str>) -> Result<Database> {
        Database::new_unsync(self, name, 0)
    }

    /// Retrieves database statistics by the given dbi.
    pub fn db_stat_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            mdbx_result(ffi::mdbx_dbi_stat(self.txn, dbi, stat.mdb_stat(), size_of::<Stat>()))?;
            Ok(stat)
        }
    }

    /// Open a new cursor on the given dbi.
    pub fn cursor_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<CursorUnsync<'_, K>> {
        CursorUnsync::new(self, dbi)
    }

    /// Returns the raw transaction pointer. For internal use only.
    pub(crate) const fn txn_ptr(&self) -> *mut ffi::MDBX_txn {
        self.txn
    }
}

impl TransactionUnsync<RW> {
    fn open_db_with_flags(&self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        Database::new_unsync(self, name, flags.bits())
    }

    /// Opens a handle to an MDBX database, creating the database if necessary.
    pub fn create_db(&self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE)
    }

    /// Stores an item into a database.
    pub fn put(
        &self,
        dbi: ffi::MDBX_dbi,
        key: impl AsRef<[u8]>,
        data: impl AsRef<[u8]>,
        flags: WriteFlags,
    ) -> Result<()> {
        let key = key.as_ref();
        let data = data.as_ref();
        let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };
        mdbx_result(unsafe {
            ffi::mdbx_put(self.txn, dbi, &key_val, &mut data_val, flags.bits())
        })?;
        Ok(())
    }

    /// Delete items from a database.
    pub fn del(
        &self,
        dbi: ffi::MDBX_dbi,
        key: impl AsRef<[u8]>,
        data: Option<&[u8]>,
    ) -> Result<bool> {
        let key = key.as_ref();
        let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let data_val: Option<ffi::MDBX_val> = data.map(|data| ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        });

        let result = if let Some(d) = data_val {
            unsafe { ffi::mdbx_del(self.txn, dbi, &key_val, &d) }
        } else {
            unsafe { ffi::mdbx_del(self.txn, dbi, &key_val, ptr::null()) }
        };

        mdbx_result(result).map(|_| true).or_else(|e| match e {
            Error::NotFound => Ok(false),
            other => Err(other),
        })
    }

    /// Empties the given database.
    pub fn clear_db(&self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(unsafe { ffi::mdbx_drop(self.txn, dbi, false) })?;
        Ok(())
    }
}

impl<K> fmt::Debug for TransactionUnsync<K>
where
    K: TransactionKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionUnsync").finish_non_exhaustive()
    }
}

impl<K> Drop for TransactionUnsync<K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        if !self.committed {
            if K::IS_READ_ONLY {
                unsafe {
                    ffi::mdbx_txn_abort(self.txn);
                }
            } else {
                let (sender, rx) = sync_channel(0);
                self.env
                    .txn_manager()
                    .send_message(TxnManagerMessage::Abort { tx: TxnPtr(self.txn), sender });
                let _ = rx.recv();
            }
        }
    }
}

// SAFETY: The transaction pointer can be sent across threads.
// The user is responsible for ensuring single-threaded access.
unsafe impl<K: TransactionKind> Send for TransactionUnsync<K> {}

/// An unsynchronized cursor for navigating database items.
///
/// This is a faster variant of [`Cursor`] that works with [`TransactionUnsync`].
pub struct CursorUnsync<'tx, K>
where
    K: TransactionKind,
{
    txn: &'tx TransactionUnsync<K>,
    cursor: *mut ffi::MDBX_cursor,
}

impl<'tx, K> CursorUnsync<'tx, K>
where
    K: TransactionKind,
{
    pub(crate) fn new(txn: &'tx TransactionUnsync<K>, dbi: ffi::MDBX_dbi) -> Result<Self> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_cursor_open(txn.txn_ptr(), dbi, &mut cursor))?;
        }
        Ok(Self { txn, cursor })
    }

    /// Returns a raw pointer to the underlying MDBX cursor.
    pub const fn cursor(&self) -> *mut ffi::MDBX_cursor {
        self.cursor
    }

    fn get<Key, Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> Result<(Option<Key>, Value, bool)>
    where
        Key: TableObject,
        Value: TableObject,
    {
        unsafe {
            let mut key_val = slice_to_val(key);
            let mut data_val = slice_to_val(data);
            let key_ptr = key_val.iov_base;
            let data_ptr = data_val.iov_base;
            let v =
                mdbx_result(ffi::mdbx_cursor_get(self.cursor, &mut key_val, &mut data_val, op))?;
            assert_ne!(data_ptr, data_val.iov_base);
            let key_out = if ptr::eq(key_ptr, key_val.iov_base) {
                None
            } else {
                Some(Key::decode_val::<K>(self.txn.txn_ptr(), key_val)?)
            };
            let data_out = Value::decode_val::<K>(self.txn.txn_ptr(), data_val)?;
            Ok((key_out, data_out, v))
        }
    }

    fn get_value<Value>(
        &mut self,
        key: Option<&[u8]>,
        data: Option<&[u8]>,
        op: MDBX_cursor_op,
    ) -> Result<Option<Value>>
    where
        Value: TableObject,
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
        Key: TableObject,
        Value: TableObject,
    {
        let (k, v, _) = mdbx_try_optional!(self.get(key, data, op));
        Ok(Some((k.unwrap(), v)))
    }

    /// Position at first key/data item.
    pub fn first<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_FIRST)
    }

    /// Position at last key/data item.
    pub fn last<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_LAST)
    }

    /// Position at next data item.
    #[allow(clippy::should_implement_trait)]
    pub fn next<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_NEXT)
    }

    /// Position at previous data item.
    pub fn prev<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_PREV)
    }

    /// Return key/data at current cursor position.
    pub fn get_current<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_GET_CURRENT)
    }

    /// Position at specified key.
    pub fn set<Value>(&mut self, key: &[u8]) -> Result<Option<Value>>
    where
        Value: TableObject,
    {
        self.get_value(Some(key), None, MDBX_SET)
    }

    /// Position at specified key, return both key and data.
    pub fn set_key<Key, Value>(&mut self, key: &[u8]) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(Some(key), None, MDBX_SET_KEY)
    }

    /// Position at first key greater than or equal to specified key.
    pub fn set_range<Key, Value>(&mut self, key: &[u8]) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(Some(key), None, MDBX_SET_RANGE)
    }

    /// DupSort-only: Position at next data item of current key.
    pub fn next_dup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_NEXT_DUP)
    }

    /// Position at first data item of next key.
    pub fn next_nodup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_NEXT_NODUP)
    }

    /// DupSort-only: Position at previous data item of current key.
    pub fn prev_dup<Key, Value>(&mut self) -> Result<Option<(Key, Value)>>
    where
        Key: TableObject,
        Value: TableObject,
    {
        self.get_full(None, None, MDBX_PREV_DUP)
    }

    /// DupSort-only: Position at last data item of current key.
    pub fn last_dup<Value>(&mut self) -> Result<Option<Value>>
    where
        Value: TableObject,
    {
        self.get_value(None, None, MDBX_LAST_DUP)
    }

    /// DupSort-only: Position at given key and at first data >= specified data.
    pub fn get_both_range<Value>(&mut self, k: &[u8], v: &[u8]) -> Result<Option<Value>>
    where
        Value: TableObject,
    {
        self.get_value(Some(k), Some(v), MDBX_GET_BOTH_RANGE)
    }
}

impl CursorUnsync<'_, RW> {
    /// Store by cursor.
    pub fn put(&mut self, key: &[u8], data: &[u8], flags: WriteFlags) -> Result<()> {
        let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
        let mut data_val =
            ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };
        mdbx_result(unsafe {
            ffi::mdbx_cursor_put(self.cursor, &key_val, &mut data_val, flags.bits())
        })?;
        Ok(())
    }

    /// Delete current key/data pair.
    pub fn del(&mut self, flags: WriteFlags) -> Result<()> {
        mdbx_result(unsafe { ffi::mdbx_cursor_del(self.cursor, flags.bits()) })?;
        Ok(())
    }
}

impl<K> Drop for CursorUnsync<'_, K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        unsafe { ffi::mdbx_cursor_close(self.cursor) }
    }
}

impl<K> fmt::Debug for CursorUnsync<'_, K>
where
    K: TransactionKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorUnsync").finish_non_exhaustive()
    }
}

// SAFETY: The cursor pointer can be sent across threads.
// The user is responsible for ensuring single-threaded access.
unsafe impl<K: TransactionKind> Send for CursorUnsync<'_, K> {}

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_send_sync<T: Send + Sync>() {}
    const fn assert_send<T: Send>() {}

    #[expect(dead_code)]
    const fn test_txn_send_sync() {
        assert_send_sync::<Transaction<RO>>();
        assert_send_sync::<Transaction<RW>>();
    }

    #[expect(dead_code)]
    const fn test_txn_unsync_send() {
        assert_send::<TransactionUnsync<RO>>();
        assert_send::<TransactionUnsync<RW>>();
    }
}
