use crate::{
    database::Database,
    environment::Environment,
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    txn_manager::{TxnManagerMessage, TxnPtr},
    Cursor, Error, Stat, TableObject,
};
use ffi::{MDBX_txn_flags_t, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE};
use parking_lot::{Mutex, MutexGuard, RwLock};
use std::{
    collections::HashMap,
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
    /// Map of DBI to subtransaction pointer for parallel writes.
    /// Only used for RW transactions with parallel writes enabled.
    subtxns: Arc<RwLock<HashMap<ffi::MDBX_dbi, SubTransaction>>>,
    /// Whether parallel writes mode is enabled.
    parallel_writes_enabled: AtomicBool,
}

/// Statistics for a parallel subtransaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubTransactionStats {
    /// Pages allocated from pre-distributed arena.
    pub arena_page_allocations: usize,
    /// Times fallback to parent was needed (arena refill events).
    pub arena_refill_events: usize,
    /// Initial pages distributed to this subtxn.
    pub arena_initial_pages: usize,
    /// Additional pages acquired from parent during fallback.
    pub arena_refill_pages: usize,
    /// Pages returned to parent on commit (not consumed).
    pub pages_unused: usize,
    /// Original arena hint for this subtxn.
    pub arena_hint: usize,
    /// DBI this subtxn is bound to.
    pub assigned_dbi: ffi::MDBX_dbi,
    /// Pages reclaimed from GC (garbage collector / freeDB).
    pub pages_from_gc: usize,
    /// Pages allocated from end-of-file (extending the database).
    pub pages_from_eof: usize,
}

/// A subtransaction for parallel writes.
/// Each subtransaction is bound to a single DBI.
#[derive(Debug)]
pub struct SubTransaction {
    /// Transaction pointer with mutex locking for thread-safety.
    txn_ptr: TransactionPtr,
    /// The DBI this subtransaction is bound to.
    dbi: ffi::MDBX_dbi,
    /// Whether this subtransaction has been committed.
    committed: AtomicBool,
}

impl SubTransaction {
    /// Creates a new subtransaction wrapper.
    fn new(ptr: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi) -> Self {
        Self { txn_ptr: TransactionPtr::new(ptr), dbi, committed: AtomicBool::new(false) }
    }

    /// Returns a clone of the transaction pointer.
    pub(crate) fn txn_ptr(&self) -> TransactionPtr {
        self.txn_ptr.clone()
    }

    /// Returns the DBI this subtransaction is bound to.
    pub fn dbi(&self) -> ffi::MDBX_dbi {
        self.dbi
    }

    /// Commits this subtransaction, merging changes to parent.
    pub fn commit(&self) -> Result<()> {
        if self.committed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.txn_ptr.txn_execute_fail_on_timeout(|ptr| {
            mdbx_result(unsafe { ffi::mdbx_subtx_commit(ptr) })
        })??;
        Ok(())
    }

    /// Aborts this subtransaction.
    pub fn abort(&self) -> Result<()> {
        if self.committed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.txn_ptr.txn_execute_fail_on_timeout(|ptr| {
            mdbx_result(unsafe { ffi::mdbx_subtx_abort(ptr) })
        })??;
        Ok(())
    }

    /// Returns statistics for this subtransaction.
    pub fn get_stats(&self) -> Result<SubTransactionStats> {
        self.txn_ptr.txn_execute_fail_on_timeout(|ptr| {
            let mut stats: ffi::MDBX_subtxn_stats = unsafe { std::mem::zeroed() };
            mdbx_result(unsafe { ffi::mdbx_subtxn_get_stats(ptr, &mut stats) })?;
            Ok(SubTransactionStats {
                arena_page_allocations: stats.arena_page_allocations,
                arena_refill_events: stats.arena_refill_events,
                arena_initial_pages: stats.arena_initial_pages,
                arena_refill_pages: stats.arena_refill_pages,
                pages_unused: stats.pages_unused,
                arena_hint: stats.arena_hint,
                assigned_dbi: stats.assigned_dbi,
                pages_from_gc: stats.pages_from_gc,
                pages_from_eof: stats.pages_from_eof,
            })
        })?
    }
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

        Self {
            inner: Arc::new(inner),
            subtxns: Arc::new(RwLock::new(HashMap::new())),
            parallel_writes_enabled: AtomicBool::new(false),
        }
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

    /// Returns whether parallel writes mode is enabled.
    ///
    /// Always returns false for read-only transactions.
    pub fn is_parallel_writes_enabled(&self) -> bool {
        if K::IS_READ_ONLY {
            false
        } else {
            self.parallel_writes_enabled.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Commits all subtransactions serially.
    ///
    /// This is a no-op for read-only transactions or if subtxns already committed.
    /// After calling this, `parallel_writes_enabled` is set to false to prevent
    /// double-commit attempts.
    pub fn commit_subtxns(&self) -> Result<()> {
        if K::IS_READ_ONLY ||
            !self.parallel_writes_enabled.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }

        let subtxns = self.subtxns.read();
        for subtxn in subtxns.values() {
            subtxn.commit()?;
        }
        Ok(())
    }

    /// Commits all subtransactions serially and returns their stats.
    ///
    /// Stats are collected BEFORE commit (commit invalidates the subtxn pointer).
    /// Returns a vector of (dbi, stats) pairs for each subtransaction.
    ///
    /// This is a no-op for read-only transactions, returning an empty vector.
    /// After calling this, `parallel_writes_enabled` is set to false to prevent
    /// double-commit attempts.
    pub fn commit_subtxns_with_stats(&self) -> Result<Vec<(ffi::MDBX_dbi, SubTransactionStats)>> {
        if K::IS_READ_ONLY ||
            !self.parallel_writes_enabled.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(Vec::new());
        }

        let subtxns = self.subtxns.read();
        let mut stats_vec = Vec::with_capacity(subtxns.len());

        for subtxn in subtxns.values() {
            let stats = subtxn.get_stats()?;
            subtxn.commit()?;
            stats_vec.push((subtxn.dbi(), stats));
        }

        Ok(stats_vec)
    }
}

impl<K> Clone for Transaction<K>
where
    K: TransactionKind,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            subtxns: Arc::clone(&self.subtxns),
            parallel_writes_enabled: AtomicBool::new(
                self.parallel_writes_enabled.load(std::sync::atomic::Ordering::SeqCst),
            ),
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

    /// Enables parallel writes mode by creating subtransactions for the given DBIs.
    ///
    /// Each DBI gets its own subtransaction that can be written to from a different thread.
    /// Cursor operations on these DBIs will automatically use the corresponding subtransaction.
    ///
    /// This requires WRITEMAP mode to be enabled on the environment.
    ///
    /// # Arguments
    /// * `dbis` - Slice of DBI handles to create subtransactions for.
    ///
    /// # Returns
    /// Ok(()) on success, or an error if subtransaction creation fails.
    pub fn enable_parallel_writes(&self, dbis: &[ffi::MDBX_dbi]) -> Result<()> {
        let specs_with_hints: Vec<_> = dbis.iter().map(|&dbi| (dbi, 0usize)).collect();
        self.enable_parallel_writes_with_hints(&specs_with_hints)
    }

    /// Enables parallel writes mode with arena size hints for specified DBIs.
    ///
    /// Similar to [`enable_parallel_writes`], but allows specifying an arena_hint
    /// for each DBI to guide page pre-allocation. An arena_hint of 0 means use
    /// equal distribution among all subtransactions.
    ///
    /// # Arguments
    /// * `specs_input` - Slice of (DBI, arena_hint) tuples.
    ///
    /// # Returns
    /// Ok(()) on success, or an error if subtransaction creation fails.
    pub fn enable_parallel_writes_with_hints(
        &self,
        specs_input: &[(ffi::MDBX_dbi, usize)],
    ) -> Result<()> {
        if specs_input.is_empty() {
            return Ok(());
        }

        // Check if already enabled
        if self.parallel_writes_enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::Incompatible);
        }

        // Debug: verify parent can read BEFORE subtxn creation
        for &(dbi, _) in specs_input {
            self.txn_execute(|txn| unsafe {
                let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
                let rc = ffi::mdbx_cursor_open(txn, dbi, &mut cursor);
                if rc == 0 {
                    ffi::mdbx_cursor_close(cursor);
                }
            })?;
        }

        // Pre-touch each DBI to ensure MAIN_DBI is dirty in parent.
        // This prevents races in subtxns when they try to modify the B-tree.
        // We do this by performing a put+delete operation which triggers cursor_touch/touch_dbi.
        for &(dbi, _) in specs_input {
            // Check if this is a DupSort table - they need special handling
            let db_flags = self.db_flags(dbi)?;
            let is_dupsort = db_flags.contains(DatabaseFlags::DUP_SORT);

            self.txn_execute(|txn| unsafe {
                let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
                let rc = ffi::mdbx_cursor_open(txn, dbi, &mut cursor);
                if rc != 0 {
                    return;
                }

                // Use a max key to touch the DBI - this won't conflict with real data
                // since it's well beyond any reasonable tx_num
                let temp_key: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
                let temp_data: [u8; 1] = [0];
                let mut key = ffi::MDBX_val {
                    iov_len: temp_key.len(),
                    iov_base: temp_key.as_ptr() as *mut c_void,
                };
                let mut data = ffi::MDBX_val {
                    iov_len: temp_data.len(),
                    iov_base: temp_data.as_ptr() as *mut c_void,
                };

                // Put triggers cursor_touch which marks MAIN_DBI as dirty.
                // For DupSort tables, use NODUPDATA to avoid adding duplicate entries
                // and properly handle the case where the key+value already exists.
                let put_flags = if is_dupsort { ffi::MDBX_NODUPDATA } else { 0 };
                let put_rc = ffi::mdbx_cursor_put(cursor, &mut key, &mut data, put_flags);

                // Delete the temp entry we just inserted (put_rc == 0 means success).
                // MDBX_KEYEXIST (-30799) means the key (or key+value for DupSort) already
                // exists, which still triggers the touch - no cleanup needed.
                if put_rc == 0 {
                    ffi::mdbx_cursor_del(cursor, 0);
                }

                ffi::mdbx_cursor_close(cursor);
            })?;
        }

        // Create specs array for the C API
        let specs: Vec<ffi::MDBX_subtxn_spec_t> = specs_input
            .iter()
            .map(|&(dbi, arena_hint)| ffi::MDBX_subtxn_spec_t { dbi, arena_hint })
            .collect();

        // Allocate space for subtransaction pointers
        let mut subtxn_ptrs: Vec<*mut ffi::MDBX_txn> = vec![ptr::null_mut(); specs_input.len()];

        // Create all subtransactions atomically
        let create_result = self.txn_execute(|parent_txn| unsafe {
            let rc = ffi::mdbx_txn_create_subtxns(
                parent_txn,
                specs.as_ptr(),
                specs.len(),
                subtxn_ptrs.as_mut_ptr(),
            );
            mdbx_result(rc)
        });
        create_result??;

        // Store subtransactions in the map
        {
            let mut subtxns = self.subtxns.write();
            for (i, &(dbi, _)) in specs_input.iter().enumerate() {
                subtxns.insert(dbi, SubTransaction::new(subtxn_ptrs[i], dbi));
            }
        }

        self.parallel_writes_enabled.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Gets the subtransaction pointer for the given DBI, if parallel writes is enabled.
    ///
    /// Returns the subtransaction pointer if one exists for this DBI, otherwise returns
    /// a clone of the parent transaction pointer.
    pub(crate) fn get_txn_ptr_for_dbi(&self, dbi: ffi::MDBX_dbi) -> TransactionPtr {
        let parallel_enabled =
            self.parallel_writes_enabled.load(std::sync::atomic::Ordering::SeqCst);
        if parallel_enabled {
            let subtxns = self.subtxns.read();
            if let Some(subtxn) = subtxns.get(&dbi) {
                return subtxn.txn_ptr();
            }
        }
        self.inner.txn.clone()
    }

    /// Aborts all subtransactions.
    ///
    /// This discards all changes made through subtransactions.
    pub fn abort_subtxns(&self) -> Result<()> {
        if !self.parallel_writes_enabled.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        let subtxns = self.subtxns.read();
        for subtxn in subtxns.values() {
            subtxn.abort()?;
        }
        Ok(())
    }

    /// Stores an item into a database, using the subtransaction if parallel writes is enabled.
    ///
    /// This is the parallel-writes-aware version of `put`.
    pub fn put_parallel(
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

        let txn_ptr = self.get_txn_ptr_for_dbi(dbi);
        mdbx_result(txn_ptr.txn_execute_fail_on_timeout(|txn| unsafe {
            ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits())
        })?)?;

        Ok(())
    }

    /// Deletes an item from a database, using the subtransaction if parallel writes is enabled.
    ///
    /// This is the parallel-writes-aware version of `del`.
    pub fn del_parallel(
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

        let txn_ptr = self.get_txn_ptr_for_dbi(dbi);
        mdbx_result(txn_ptr.txn_execute_fail_on_timeout(|txn| {
            if let Some(d) = data_val {
                unsafe { ffi::mdbx_del(txn, dbi, &key_val, &d) }
            } else {
                unsafe { ffi::mdbx_del(txn, dbi, &key_val, ptr::null()) }
            }
        })?)
        .map(|_| true)
        .or_else(|e| match e {
            Error::NotFound => Ok(false),
            other => Err(other),
        })
    }

    /// Opens a cursor on the given DBI, using the subtransaction if parallel writes is enabled.
    ///
    /// This is the parallel-writes-aware version of `cursor_with_dbi`.
    pub fn cursor_with_dbi_parallel(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<RW>> {
        let txn_ptr = self.get_txn_ptr_for_dbi(dbi);
        Cursor::new_with_ptr(self.clone(), dbi, txn_ptr)
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

    /// Returns the raw transaction pointer.
    pub(crate) fn as_ptr(&self) -> *mut ffi::MDBX_txn {
        self.txn
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

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_send_sync<T: Send + Sync>() {}

    #[expect(dead_code)]
    const fn test_txn_send_sync() {
        assert_send_sync::<Transaction<RO>>();
        assert_send_sync::<Transaction<RW>>();
    }
}
