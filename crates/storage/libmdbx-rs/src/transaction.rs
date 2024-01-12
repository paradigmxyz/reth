use crate::{
    database::Database,
    environment::{Environment, TxnManagerMessage, TxnPtr},
    error::{mdbx_result, Result},
    flags::{DatabaseFlags, WriteFlags},
    Cursor, Error, Stat, TableObject,
};
use ffi::{MDBX_txn_flags_t, MDBX_TXN_RDONLY, MDBX_TXN_READWRITE};
use indexmap::IndexSet;
use libc::{c_uint, c_void};
use parking_lot::Mutex;
use std::{
    fmt,
    fmt::Debug,
    mem::size_of,
    ptr, slice,
    sync::{atomic::AtomicBool, mpsc::sync_channel, Arc},
    time::Duration,
};

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for RO {}
    impl Sealed for RW {}
}

pub trait TransactionKind: private::Sealed + Send + Sync + Debug + 'static {
    #[doc(hidden)]
    const ONLY_CLEAN: bool;

    #[doc(hidden)]
    const OPEN_FLAGS: MDBX_txn_flags_t;

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
    const ONLY_CLEAN: bool = true;
    const OPEN_FLAGS: MDBX_txn_flags_t = MDBX_TXN_RDONLY;
    const IS_READ_ONLY: bool = true;
}
impl TransactionKind for RW {
    const ONLY_CLEAN: bool = false;
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

    pub(crate) fn new_from_ptr(env: Environment, txn: *mut ffi::MDBX_txn) -> Self {
        let inner = TransactionInner {
            txn: TransactionPtr::new(txn),
            primed_dbis: Mutex::new(IndexSet::new()),
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
    pub(crate) fn txn_execute<F, T>(&self, f: F) -> T
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.inner.txn_execute(f)
    }

    /// Returns a copy of the raw pointer to the underlying MDBX transaction.
    #[doc(hidden)]
    pub fn txn(&self) -> *mut ffi::MDBX_txn {
        self.inner.txn.txn
    }

    /// Executes the given closure once
    ///
    /// This is only intended to be used when accessing mdbx ffi functions directly is required.
    ///
    /// The caller **must** ensure that the pointer is only used within the closure.
    #[inline]
    #[doc(hidden)]
    pub fn with_raw_tx_ptr<F, T>(&self, f: F) -> T
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        let _lock = self.inner.txn.lock.lock();
        f(self.inner.txn.txn)
    }

    /// Returns a raw pointer to the MDBX environment.
    pub fn env(&self) -> &Environment {
        &self.inner.env
    }

    /// Returns the transaction id.
    pub fn id(&self) -> u64 {
        self.txn_execute(|txn| unsafe { ffi::mdbx_txn_id(txn) })
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the
    /// database. If the database supports duplicate keys
    /// ([DatabaseFlags::DUP_SORT]) then the first data item for the key will be
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
        })
    }

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    pub fn commit(self) -> Result<(bool, CommitLatency)> {
        self.commit_and_rebind_open_dbs().map(|v| (v.0, v.1))
    }

    pub fn prime_for_permaopen(&self, db: Database) {
        self.inner.primed_dbis.lock().insert(db.dbi());
    }

    /// Commits the transaction and returns table handles permanently open until dropped.
    pub fn commit_and_rebind_open_dbs(self) -> Result<(bool, CommitLatency, Vec<Database>)> {
        let result = {
            let result = self.txn_execute(|txn| {
                if K::ONLY_CLEAN {
                    let mut latency = CommitLatency::new();
                    mdbx_result(unsafe {
                        ffi::mdbx_txn_commit_ex(txn, latency.mdb_commit_latency())
                    })
                    .map(|v| (v, latency))
                } else {
                    let (sender, rx) = sync_channel(0);
                    self.env()
                        .ensure_txn_manager()
                        .unwrap()
                        .send(TxnManagerMessage::Commit { tx: TxnPtr(txn), sender })
                        .unwrap();
                    rx.recv().unwrap()
                }
            });
            self.inner.set_committed();
            result
        };
        result.map(|(v, latency)| {
            (
                v,
                latency,
                self.inner
                    .primed_dbis
                    .lock()
                    .iter()
                    .map(|&dbi| Database::new_from_ptr(dbi, self.env().clone()))
                    .collect(),
            )
        })
    }

    /// Opens a handle to an MDBX database.
    ///
    /// If `name` is [None], then the returned handle will be for the default database.
    ///
    /// If `name` is not [None], then the returned handle will be for a named database. In this
    /// case the environment must be configured to allow named databases through
    /// [EnvironmentBuilder::set_max_dbs()](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// The returned database handle may be shared among any transaction in the environment.
    ///
    /// The database name may not contain the null character.
    pub fn open_db(&self, name: Option<&str>) -> Result<Database> {
        Database::new(self, name, 0)
    }

    /// Gets the option flags for the given database in the transaction.
    pub fn db_flags(&self, db: &Database) -> Result<DatabaseFlags> {
        let mut flags: c_uint = 0;
        unsafe {
            mdbx_result(self.txn_execute(|txn| {
                ffi::mdbx_dbi_flags_ex(txn, db.dbi(), &mut flags, ptr::null_mut())
            }))?;
        }

        // The types are not the same on Windows. Great!
        #[cfg_attr(not(windows), allow(clippy::useless_conversion))]
        Ok(DatabaseFlags::from_bits_truncate(flags.try_into().unwrap()))
    }

    /// Retrieves database statistics.
    pub fn db_stat(&self, db: &Database) -> Result<Stat> {
        self.db_stat_with_dbi(db.dbi())
    }

    /// Retrieves database statistics by the given dbi.
    pub fn db_stat_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            mdbx_result(self.txn_execute(|txn| {
                ffi::mdbx_dbi_stat(txn, dbi, stat.mdb_stat(), size_of::<Stat>())
            }))?;
            Ok(stat)
        }
    }

    /// Open a new cursor on the given database.
    pub fn cursor(&self, db: &Database) -> Result<Cursor<K>> {
        Cursor::new(self.clone(), db.dbi())
    }

    /// Open a new cursor on the given dbi.
    pub fn cursor_with_dbi(&self, dbi: ffi::MDBX_dbi) -> Result<Cursor<K>> {
        Cursor::new(self.clone(), dbi)
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
    /// A set of database handles that are primed for permaopen.
    primed_dbis: Mutex<IndexSet<ffi::MDBX_dbi>>,
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
    fn txn_execute<F, T>(&self, f: F) -> T
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        self.txn.txn_execute(f)
    }
}

impl<K> Drop for TransactionInner<K>
where
    K: TransactionKind,
{
    fn drop(&mut self) {
        self.txn_execute(|txn| {
            if !self.has_committed() {
                if K::ONLY_CLEAN {
                    unsafe {
                        ffi::mdbx_txn_abort(txn);
                    }
                } else {
                    let (sender, rx) = sync_channel(0);
                    self.env
                        .ensure_txn_manager()
                        .unwrap()
                        .send(TxnManagerMessage::Abort { tx: TxnPtr(txn), sender })
                        .unwrap();
                    rx.recv().unwrap().unwrap();
                }
            }
        })
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
    /// [EnvironmentBuilder::set_max_dbs()](crate::EnvironmentBuilder::set_max_dbs).
    ///
    /// This function will fail with [Error::BadRslot] if called by a thread with an open
    /// transaction.
    pub fn create_db(&self, name: Option<&str>, flags: DatabaseFlags) -> Result<Database> {
        self.open_db_with_flags(name, flags | DatabaseFlags::CREATE)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default
    /// behavior is to enter the new key/data pair, replacing any previously
    /// existing key if duplicates are disallowed, or adding a duplicate data
    /// item if duplicates are allowed ([DatabaseFlags::DUP_SORT]).
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
        }))?;

        Ok(())
    }

    /// Returns a buffer which can be used to write a value into the item at the
    /// given key and with the given length. The buffer must be completely
    /// filled by the caller.
    pub fn reserve(
        &self,
        db: &Database,
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
                ffi::mdbx_put(
                    txn,
                    db.dbi(),
                    &key_val,
                    &mut data_val,
                    flags.bits() | ffi::MDBX_RESERVE,
                )
            }))?;
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
            })
        })
        .map(|_| true)
        .or_else(|e| match e {
            Error::NotFound => Ok(false),
            other => Err(other),
        })
    }

    /// Empties the given database. All items will be removed.
    pub fn clear_db(&self, dbi: ffi::MDBX_dbi) -> Result<()> {
        mdbx_result(self.txn_execute(|txn| unsafe { ffi::mdbx_drop(txn, dbi, false) }))?;

        Ok(())
    }

    /// Drops the database from the environment.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi
    /// BEFORE calling this function.
    pub unsafe fn drop_db(&self, db: Database) -> Result<()> {
        mdbx_result(self.txn_execute(|txn| ffi::mdbx_drop(txn, db.dbi(), true)))?;

        Ok(())
    }
}

impl Transaction<RO> {
    /// Closes the database handle.
    ///
    /// # Safety
    /// Caller must close ALL other [Database] and [Cursor] instances pointing to the same dbi
    /// BEFORE calling this function.
    pub unsafe fn close_db(&self, db: Database) -> Result<()> {
        mdbx_result(ffi::mdbx_dbi_close(self.env().env_ptr(), db.dbi()))?;

        Ok(())
    }
}

impl Transaction<RW> {
    /// Begins a new nested transaction inside of this transaction.
    pub fn begin_nested_txn(&mut self) -> Result<Transaction<RW>> {
        if self.inner.env.is_write_map() {
            return Err(Error::NestedTransactionsUnsupportedWithWriteMap)
        }
        self.txn_execute(|txn| {
            let (tx, rx) = sync_channel(0);
            self.env()
                .ensure_txn_manager()
                .unwrap()
                .send(TxnManagerMessage::Begin {
                    parent: TxnPtr(txn),
                    flags: RW::OPEN_FLAGS,
                    sender: tx,
                })
                .unwrap();

            rx.recv().unwrap().map(|ptr| Transaction::new_from_ptr(self.env().clone(), ptr.0))
        })
    }
}

/// A shareable pointer to an MDBX transaction.
#[derive(Clone)]
pub(crate) struct TransactionPtr {
    txn: *mut ffi::MDBX_txn,
    lock: Arc<Mutex<()>>,
}

impl TransactionPtr {
    fn new(txn: *mut ffi::MDBX_txn) -> Self {
        Self { txn, lock: Arc::new(Mutex::new(())) }
    }

    /// Executes the given closure once the lock on the transaction is acquired.
    #[inline]
    pub(crate) fn txn_execute<F, T>(&self, f: F) -> T
    where
        F: FnOnce(*mut ffi::MDBX_txn) -> T,
    {
        let _lck = self.lock.lock();
        (f)(self.txn)
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
    /// Create a new CommitLatency with zero'd inner struct `ffi::MDBX_commit_latency`.
    pub(crate) fn new() -> Self {
        unsafe { Self(std::mem::zeroed()) }
    }

    /// Returns a mut pointer to `ffi::MDBX_commit_latency`.
    pub(crate) fn mdb_commit_latency(&mut self) -> *mut ffi::MDBX_commit_latency {
        &mut self.0
    }
}

impl CommitLatency {
    /// Duration of preparation (commit child transactions, update
    /// sub-databases records and cursors destroying).
    #[inline]
    pub fn preparation(&self) -> Duration {
        Self::time_to_duration(self.0.preparation)
    }

    /// Duration of GC update by wall clock.
    #[inline]
    pub fn gc_wallclock(&self) -> Duration {
        Self::time_to_duration(self.0.gc_wallclock)
    }

    /// Duration of internal audit if enabled.
    #[inline]
    pub fn audit(&self) -> Duration {
        Self::time_to_duration(self.0.audit)
    }

    /// Duration of writing dirty/modified data pages to a filesystem,
    /// i.e. the summary duration of a `write()` syscalls during commit.
    #[inline]
    pub fn write(&self) -> Duration {
        Self::time_to_duration(self.0.write)
    }

    /// Duration of syncing written data to the disk/storage, i.e.
    /// the duration of a `fdatasync()` or a `msync()` syscall during commit.
    #[inline]
    pub fn sync(&self) -> Duration {
        Self::time_to_duration(self.0.sync)
    }

    /// Duration of transaction ending (releasing resources).
    #[inline]
    pub fn ending(&self) -> Duration {
        Self::time_to_duration(self.0.ending)
    }

    /// The total duration of a commit.
    #[inline]
    pub fn whole(&self) -> Duration {
        Self::time_to_duration(self.0.whole)
    }

    /// User-mode CPU time spent on GC update.
    #[inline]
    pub fn gc_cputime(&self) -> Duration {
        Self::time_to_duration(self.0.gc_cputime)
    }

    #[inline]
    fn time_to_duration(time: u32) -> Duration {
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

    fn assert_send_sync<T: Send + Sync>() {}

    #[allow(dead_code)]
    fn test_txn_send_sync() {
        assert_send_sync::<Transaction<RO>>();
        assert_send_sync::<Transaction<RW>>();
    }
}
