use crate::{
    database::Database,
    error::{mdbx_result, Error, Result},
    flags::EnvironmentFlags,
    transaction::{RO, RW},
    txn_manager::{TxnManager, TxnManagerMessage, TxnPtr},
    Transaction, TransactionKind,
};
use byteorder::{ByteOrder, NativeEndian};
use mem::size_of;
use std::{
    ffi::CString,
    fmt,
    fmt::Debug,
    mem,
    ops::{Bound, RangeBounds},
    path::Path,
    ptr,
    sync::{mpsc::sync_channel, Arc},
    thread::sleep,
    time::Duration,
};

/// The default maximum duration of a read transaction.
#[cfg(feature = "read-tx-timeouts")]
const DEFAULT_MAX_READ_TRANSACTION_DURATION: Duration = Duration::from_secs(5 * 60);

/// An environment supports multiple databases, all residing in the same shared-memory map.
///
/// Accessing the environment is thread-safe.
/// The environment will be closed when the last instance of this type is dropped.
#[derive(Clone)]
pub struct Environment {
    inner: Arc<EnvironmentInner>,
}

impl Environment {
    /// Creates a new builder for specifying options for opening an MDBX environment.
    pub fn builder() -> EnvironmentBuilder {
        EnvironmentBuilder {
            flags: EnvironmentFlags::default(),
            max_readers: None,
            max_dbs: None,
            rp_augment_limit: None,
            loose_limit: None,
            dp_reserve_limit: None,
            txn_dp_limit: None,
            spill_max_denominator: None,
            spill_min_denominator: None,
            geometry: None,
            log_level: None,
            kind: Default::default(),
            #[cfg(not(windows))]
            handle_slow_readers: None,
            #[cfg(feature = "read-tx-timeouts")]
            max_read_transaction_duration: None,
        }
    }

    /// Returns true if the environment was opened as WRITEMAP.
    #[inline]
    pub fn is_write_map(&self) -> bool {
        self.inner.env_kind.is_write_map()
    }

    /// Returns the kind of the environment.
    #[inline]
    pub fn env_kind(&self) -> EnvironmentKind {
        self.inner.env_kind
    }

    /// Returns true if the environment was opened in [crate::Mode::ReadWrite] mode.
    #[inline]
    pub fn is_read_write(&self) -> bool {
        self.inner.env_kind.is_write_map()
    }

    /// Returns true if the environment was opened in [crate::Mode::ReadOnly] mode.
    #[inline]
    pub fn is_read_only(&self) -> bool {
        !self.inner.env_kind.is_write_map()
    }

    /// Returns the transaction manager.
    #[inline]
    pub(crate) fn txn_manager(&self) -> &TxnManager {
        &self.inner.txn_manager
    }

    /// Create a read-only transaction for use with the environment.
    #[inline]
    pub fn begin_ro_txn(&self) -> Result<Transaction<RO>> {
        Transaction::new(self.clone())
    }

    /// Create a read-write transaction for use with the environment. This method will block while
    /// there are any other read-write transactions open on the environment.
    pub fn begin_rw_txn(&self) -> Result<Transaction<RW>> {
        let txn = loop {
            let (tx, rx) = sync_channel(0);
            self.txn_manager().send_message(TxnManagerMessage::Begin {
                parent: TxnPtr(ptr::null_mut()),
                flags: RW::OPEN_FLAGS,
                sender: tx,
            });
            let res = rx.recv().unwrap();
            if let Err(Error::Busy) = &res {
                sleep(Duration::from_millis(250));
                continue
            }

            break res
        }?;
        Ok(Transaction::new_from_ptr(self.clone(), txn.0))
    }

    /// Returns a raw pointer to the underlying MDBX environment.
    ///
    /// The caller **must** ensure that the pointer is never dereferenced after the environment has
    /// been dropped.
    #[inline]
    pub(crate) fn env_ptr(&self) -> *mut ffi::MDBX_env {
        self.inner.env
    }

    /// Executes the given closure once
    ///
    /// This is only intended to be used when accessing mdbx ffi functions directly is required.
    ///
    /// The caller **must** ensure that the pointer is only used within the closure.
    #[inline]
    #[doc(hidden)]
    pub fn with_raw_env_ptr<F, T>(&self, f: F) -> T
    where
        F: FnOnce(*mut ffi::MDBX_env) -> T,
    {
        (f)(self.env_ptr())
    }

    /// Flush the environment data buffers to disk.
    pub fn sync(&self, force: bool) -> Result<bool> {
        mdbx_result(unsafe { ffi::mdbx_env_sync_ex(self.env_ptr(), force, false) })
    }

    /// Retrieves statistics about this environment.
    pub fn stat(&self) -> Result<Stat> {
        unsafe {
            let mut stat = Stat::new();
            mdbx_result(ffi::mdbx_env_stat_ex(
                self.env_ptr(),
                ptr::null(),
                stat.mdb_stat(),
                size_of::<Stat>(),
            ))?;
            Ok(stat)
        }
    }

    /// Retrieves info about this environment.
    pub fn info(&self) -> Result<Info> {
        unsafe {
            let mut info = Info(mem::zeroed());
            mdbx_result(ffi::mdbx_env_info_ex(
                self.env_ptr(),
                ptr::null(),
                &mut info.0,
                size_of::<Info>(),
            ))?;
            Ok(info)
        }
    }

    /// Retrieves the total number of pages on the freelist.
    ///
    /// Along with [Environment::info()], this can be used to calculate the exact number
    /// of used pages as well as free pages in this environment.
    ///
    /// ```
    /// # use reth_libmdbx::Environment;
    /// let dir = tempfile::tempdir().unwrap();
    /// let env = Environment::builder().open(dir.path()).unwrap();
    /// let info = env.info().unwrap();
    /// let stat = env.stat().unwrap();
    /// let freelist = env.freelist().unwrap();
    /// let last_pgno = info.last_pgno() + 1; // pgno is 0 based.
    /// let total_pgs = info.map_size() / stat.page_size() as usize;
    /// let pgs_in_use = last_pgno - freelist;
    /// let pgs_free = total_pgs - pgs_in_use;
    /// ```
    ///
    /// Note:
    ///
    /// * MDBX stores all the freelists in the designated database 0 in each environment, and the
    ///   freelist count is stored at the beginning of the value as `libc::uint32_t` in the native
    ///   byte order.
    ///
    /// * It will create a read transaction to traverse the freelist database.
    pub fn freelist(&self) -> Result<usize> {
        let mut freelist: usize = 0;
        let txn = self.begin_ro_txn()?;
        let db = Database::freelist_db();
        let cursor = txn.cursor(&db)?;

        for result in cursor.iter_slices() {
            let (_key, value) = result?;
            if value.len() < size_of::<usize>() {
                return Err(Error::Corrupted)
            }

            let s = &value[..size_of::<usize>()];
            freelist += NativeEndian::read_u32(s) as usize;
        }

        Ok(freelist)
    }
}

/// Container type for Environment internals.
///
/// This holds the raw pointer to the MDBX environment and the transaction manager.
/// The env is opened via [mdbx_env_create](ffi::mdbx_env_create) and closed when this type drops.
struct EnvironmentInner {
    /// The raw pointer to the MDBX environment.
    ///
    /// Accessing the environment is thread-safe as long as long as this type exists.
    env: *mut ffi::MDBX_env,
    /// Whether the environment was opened as WRITEMAP.
    env_kind: EnvironmentKind,
    /// Transaction manager
    txn_manager: TxnManager,
}

impl Drop for EnvironmentInner {
    fn drop(&mut self) {
        // Close open mdbx environment on drop
        unsafe {
            ffi::mdbx_env_close_ex(self.env, false);
        }
    }
}

// SAFETY: internal type, only used inside [Environment]. Accessing the environment pointer is
// thread-safe
unsafe impl Send for EnvironmentInner {}
unsafe impl Sync for EnvironmentInner {}

/// Determines how data is mapped into memory
///
/// It only takes affect when the environment is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvironmentKind {
    /// Open the environment in default mode, without WRITEMAP.
    #[default]
    Default,
    /// Open the environment as mdbx-WRITEMAP.
    /// Use a writeable memory map unless the environment is opened as MDBX_RDONLY
    /// ([crate::Mode::ReadOnly]).
    ///
    /// All data will be mapped into memory in the read-write mode [crate::Mode::ReadWrite]. This
    /// offers a significant performance benefit, since the data will be modified directly in
    /// mapped memory and then flushed to disk by single system call, without any memory
    /// management nor copying.
    ///
    /// This mode is incompatible with nested transactions.
    WriteMap,
}

impl EnvironmentKind {
    /// Returns true if the environment was opened as WRITEMAP.
    #[inline]
    pub const fn is_write_map(&self) -> bool {
        matches!(self, EnvironmentKind::WriteMap)
    }

    /// Additional flags required when opening the environment.
    pub(crate) fn extra_flags(&self) -> ffi::MDBX_env_flags_t {
        match self {
            EnvironmentKind::Default => ffi::MDBX_ENV_DEFAULTS,
            EnvironmentKind::WriteMap => ffi::MDBX_WRITEMAP,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct EnvPtr(pub(crate) *mut ffi::MDBX_env);
unsafe impl Send for EnvPtr {}
unsafe impl Sync for EnvPtr {}

/// Environment statistics.
///
/// Contains information about the size and layout of an MDBX environment or database.
#[derive(Debug)]
#[repr(transparent)]
pub struct Stat(ffi::MDBX_stat);

impl Stat {
    /// Create a new Stat with zero'd inner struct `ffi::MDB_stat`.
    pub(crate) fn new() -> Stat {
        unsafe { Stat(mem::zeroed()) }
    }

    /// Returns a mut pointer to `ffi::MDB_stat`.
    pub(crate) fn mdb_stat(&mut self) -> *mut ffi::MDBX_stat {
        &mut self.0
    }
}

impl Stat {
    /// Size of a database page. This is the same for all databases in the environment.
    #[inline]
    pub fn page_size(&self) -> u32 {
        self.0.ms_psize
    }

    /// Depth (height) of the B-tree.
    #[inline]
    pub fn depth(&self) -> u32 {
        self.0.ms_depth
    }

    /// Number of internal (non-leaf) pages.
    #[inline]
    pub fn branch_pages(&self) -> usize {
        self.0.ms_branch_pages as usize
    }

    /// Number of leaf pages.
    #[inline]
    pub fn leaf_pages(&self) -> usize {
        self.0.ms_leaf_pages as usize
    }

    /// Number of overflow pages.
    #[inline]
    pub fn overflow_pages(&self) -> usize {
        self.0.ms_overflow_pages as usize
    }

    /// Number of data items.
    #[inline]
    pub fn entries(&self) -> usize {
        self.0.ms_entries as usize
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct GeometryInfo(ffi::MDBX_envinfo__bindgen_ty_1);

impl GeometryInfo {
    pub fn min(&self) -> u64 {
        self.0.lower
    }
}

/// Environment information.
///
/// Contains environment information about the map size, readers, last txn id etc.
#[derive(Debug)]
#[repr(transparent)]
pub struct Info(ffi::MDBX_envinfo);

impl Info {
    pub fn geometry(&self) -> GeometryInfo {
        GeometryInfo(self.0.mi_geo)
    }

    /// Size of memory map.
    #[inline]
    pub fn map_size(&self) -> usize {
        self.0.mi_mapsize as usize
    }

    /// Last used page number
    #[inline]
    pub fn last_pgno(&self) -> usize {
        self.0.mi_last_pgno as usize
    }

    /// Last transaction ID
    #[inline]
    pub fn last_txnid(&self) -> usize {
        self.0.mi_recent_txnid as usize
    }

    /// Max reader slots in the environment
    #[inline]
    pub fn max_readers(&self) -> usize {
        self.0.mi_maxreaders as usize
    }

    /// Max reader slots used in the environment
    #[inline]
    pub fn num_readers(&self) -> usize {
        self.0.mi_numreaders as usize
    }

    /// Return the internal page ops metrics
    #[inline]
    pub fn page_ops(&self) -> PageOps {
        PageOps {
            newly: self.0.mi_pgop_stat.newly,
            cow: self.0.mi_pgop_stat.cow,
            clone: self.0.mi_pgop_stat.clone,
            split: self.0.mi_pgop_stat.split,
            merge: self.0.mi_pgop_stat.merge,
            spill: self.0.mi_pgop_stat.spill,
            unspill: self.0.mi_pgop_stat.unspill,
            wops: self.0.mi_pgop_stat.wops,
            prefault: self.0.mi_pgop_stat.prefault,
            mincore: self.0.mi_pgop_stat.mincore,
            msync: self.0.mi_pgop_stat.msync,
            fsync: self.0.mi_pgop_stat.fsync,
        }
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment").field("kind", &self.inner.env_kind).finish_non_exhaustive()
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Environment Builder
///////////////////////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageSize {
    MinimalAcceptable,
    Set(usize),
}

/// Statistics of page operations overall of all (running, completed and aborted) transactions
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOps {
    /// Quantity of a new pages added
    pub newly: u64,
    /// Quantity of pages copied for update
    pub cow: u64,
    /// Quantity of parent's dirty pages clones for nested transactions
    pub clone: u64,
    /// Page splits
    pub split: u64,
    /// Page merges
    pub merge: u64,
    /// Quantity of spilled dirty pages
    pub spill: u64,
    /// Quantity of unspilled/reloaded pages
    pub unspill: u64,
    /// Number of explicit write operations (not a pages) to a disk
    pub wops: u64,
    /// Number of explicit msync/flush-to-disk operations
    pub msync: u64,
    /// Number of explicit fsync/flush-to-disk operations
    pub fsync: u64,
    /// Number of prefault write operations
    pub prefault: u64,
    /// Number of mincore() calls
    pub mincore: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Geometry<R> {
    pub size: Option<R>,
    pub growth_step: Option<isize>,
    pub shrink_threshold: Option<isize>,
    pub page_size: Option<PageSize>,
}

impl<R> Default for Geometry<R> {
    fn default() -> Self {
        Self { size: None, growth_step: None, shrink_threshold: None, page_size: None }
    }
}

/// Handle-Slow-Readers callback function to resolve database full/overflow issue due to a reader(s)
/// which prevents the old data from being recycled.
///
/// Read transactions prevent reuse of pages freed by newer write transactions, thus the database
/// can grow quickly. This callback will be called when there is not enough space in the database
/// (i.e. before increasing the database size or before `MDBX_MAP_FULL` error) and thus can be
/// used to resolve issues with a "long-lived" read transacttions.
///
/// Depending on the arguments and needs, your implementation may wait,
/// terminate a process or thread that is performing a long read, or perform
/// some other action. In doing so it is important that the returned code always
/// corresponds to the performed action.
///
/// # Arguments
///
/// * `process_id` – A proceess id of the reader process.
/// * `thread_id` – A thread id of the reader thread.
/// * `read_txn_id` – An oldest read transaction number on which stalled.
/// * `gap` – A lag from the last committed txn.
/// * `space` – A space that actually become available for reuse after this reader finished. The
///   callback function can take this value into account to evaluate the impact that a long-running
///   transaction has.
/// * `retry` – A retry number starting from 0. If callback has returned 0 at least once, then at
///   end of current handling loop the callback function will be called additionally with negative
///   `retry` value to notify about the end of loop. The callback function can use this fact to
///   implement timeout reset logic while waiting for a readers.
///
/// # Returns
/// A return code that determines the further actions for MDBX and must match the action which
/// was executed by the callback:
/// * `-2` or less – An error condition and the reader was not killed.
/// * `-1` – The callback was unable to solve the problem and agreed on `MDBX_MAP_FULL` error; MDBX
///   should increase the database size or return `MDBX_MAP_FULL` error.
/// * `0` – The callback solved the problem or just waited for a while, libmdbx should rescan the
///   reader lock table and retry. This also includes a situation when corresponding transaction
///   terminated in normal way by `mdbx_txn_abort()` or `mdbx_txn_reset()`, and may be restarted.
///   I.e. reader slot isn't needed to be cleaned from transaction.
/// * `1` – Transaction aborted asynchronous and reader slot should be cleared immediately, i.e.
///   read transaction will not continue but `mdbx_txn_abort()` nor `mdbx_txn_reset()` will be
///   called later.
/// * `2` or greater – The reader process was terminated or killed, and MDBX should entirely reset
///   reader registration.
pub type HandleSlowReadersCallback = fn(
    process_id: u32,
    thread_id: u32,
    read_txn_id: u64,
    gap: usize,
    space: usize,
    retry: isize,
) -> HandleSlowReadersReturnCode;

#[derive(Debug)]
pub enum HandleSlowReadersReturnCode {
    /// An error condition and the reader was not killed.
    Error,
    /// The callback was unable to solve the problem and agreed on `MDBX_MAP_FULL` error;
    /// MDBX should increase the database size or return `MDBX_MAP_FULL` error.
    ProceedWithoutKillingReader,
    /// The callback solved the problem or just waited for a while, libmdbx should rescan the
    /// reader lock table and retry. This also includes a situation when corresponding transaction
    /// terminated in normal way by `mdbx_txn_abort()` or `mdbx_txn_reset()`, and may be restarted.
    /// I.e. reader slot isn't needed to be cleaned from transaction.
    Success,
    /// Transaction aborted asynchronous and reader slot should be cleared immediately, i.e. read
    /// transaction will not continue but `mdbx_txn_abort()` nor `mdbx_txn_reset()` will be called
    /// later.
    ClearReaderSlot,
    /// The reader process was terminated or killed, and MDBX should entirely reset reader
    /// registration.
    ReaderProcessTerminated,
}

impl From<HandleSlowReadersReturnCode> for i32 {
    fn from(value: HandleSlowReadersReturnCode) -> Self {
        match value {
            HandleSlowReadersReturnCode::Error => -2,
            HandleSlowReadersReturnCode::ProceedWithoutKillingReader => -1,
            HandleSlowReadersReturnCode::Success => 0,
            HandleSlowReadersReturnCode::ClearReaderSlot => 1,
            HandleSlowReadersReturnCode::ReaderProcessTerminated => 2,
        }
    }
}

/// Options for opening or creating an environment.
#[derive(Debug, Clone)]
pub struct EnvironmentBuilder {
    flags: EnvironmentFlags,
    max_readers: Option<u64>,
    max_dbs: Option<u64>,
    rp_augment_limit: Option<u64>,
    loose_limit: Option<u64>,
    dp_reserve_limit: Option<u64>,
    txn_dp_limit: Option<u64>,
    spill_max_denominator: Option<u64>,
    spill_min_denominator: Option<u64>,
    geometry: Option<Geometry<(Option<usize>, Option<usize>)>>,
    log_level: Option<ffi::MDBX_log_level_t>,
    kind: EnvironmentKind,
    #[cfg(not(windows))]
    handle_slow_readers: Option<HandleSlowReadersCallback>,
    #[cfg(feature = "read-tx-timeouts")]
    /// The maximum duration of a read transaction. If [None], but the `read-tx-timeout` feature is
    /// enabled, the default value of [DEFAULT_MAX_READ_TRANSACTION_DURATION] is used.
    max_read_transaction_duration: Option<read_transactions::MaxReadTransactionDuration>,
}

impl EnvironmentBuilder {
    /// Open an environment.
    ///
    /// Database files will be opened with 644 permissions.
    pub fn open(&self, path: &Path) -> Result<Environment> {
        self.open_with_permissions(path, 0o644)
    }

    /// Open an environment with the provided UNIX permissions.
    ///
    /// The path may not contain the null character.
    pub fn open_with_permissions(
        &self,
        path: &Path,
        mode: ffi::mdbx_mode_t,
    ) -> Result<Environment> {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        unsafe {
            if let Some(log_level) = self.log_level {
                // Returns the previously debug_flags in the 0-15 bits and log_level in the
                // 16-31 bits, no need to use `mdbx_result`.
                ffi::mdbx_setup_debug(log_level, ffi::MDBX_DBG_DONTCHANGE, None);
            }

            mdbx_result(ffi::mdbx_env_create(&mut env))?;

            if let Err(e) = (|| {
                if let Some(geometry) = &self.geometry {
                    let mut min_size = -1;
                    let mut max_size = -1;

                    if let Some(size) = geometry.size {
                        if let Some(size) = size.0 {
                            min_size = size as isize;
                        }

                        if let Some(size) = size.1 {
                            max_size = size as isize;
                        }
                    }

                    mdbx_result(ffi::mdbx_env_set_geometry(
                        env,
                        min_size,
                        -1,
                        max_size,
                        geometry.growth_step.unwrap_or(-1),
                        geometry.shrink_threshold.unwrap_or(-1),
                        match geometry.page_size {
                            None => -1,
                            Some(PageSize::MinimalAcceptable) => 0,
                            Some(PageSize::Set(size)) => size as isize,
                        },
                    ))?;
                }
                for (opt, v) in [
                    (ffi::MDBX_opt_max_db, self.max_dbs),
                    (ffi::MDBX_opt_rp_augment_limit, self.rp_augment_limit),
                    (ffi::MDBX_opt_loose_limit, self.loose_limit),
                    (ffi::MDBX_opt_dp_reserve_limit, self.dp_reserve_limit),
                    (ffi::MDBX_opt_txn_dp_limit, self.txn_dp_limit),
                    (ffi::MDBX_opt_spill_max_denominator, self.spill_max_denominator),
                    (ffi::MDBX_opt_spill_min_denominator, self.spill_min_denominator),
                ] {
                    if let Some(v) = v {
                        mdbx_result(ffi::mdbx_env_set_option(env, opt, v))?;
                    }
                }

                // set max readers if specified
                if let Some(max_readers) = self.max_readers {
                    mdbx_result(ffi::mdbx_env_set_option(
                        env,
                        ffi::MDBX_opt_max_readers,
                        max_readers,
                    ))?;
                }

                #[cfg(not(windows))]
                if let Some(handle_slow_readers) = self.handle_slow_readers {
                    mdbx_result(ffi::mdbx_env_set_hsr(
                        env,
                        handle_slow_readers_callback(handle_slow_readers),
                    ))?;
                }

                #[cfg(unix)]
                fn path_to_bytes<P: AsRef<Path>>(path: P) -> Vec<u8> {
                    use std::os::unix::ffi::OsStrExt;
                    path.as_ref().as_os_str().as_bytes().to_vec()
                }

                #[cfg(windows)]
                fn path_to_bytes<P: AsRef<Path>>(path: P) -> Vec<u8> {
                    // On Windows, could use std::os::windows::ffi::OsStrExt to encode_wide(),
                    // but we end up with a Vec<u16> instead of a Vec<u8>, so that doesn't
                    // really help.
                    path.as_ref().to_string_lossy().to_string().into_bytes()
                }

                let path = match CString::new(path_to_bytes(path)) {
                    Ok(path) => path,
                    Err(_) => return Err(Error::Invalid),
                };
                mdbx_result(ffi::mdbx_env_open(
                    env,
                    path.as_ptr(),
                    self.flags.make_flags() | self.kind.extra_flags(),
                    mode,
                ))?;

                Ok(())
            })() {
                ffi::mdbx_env_close_ex(env, false);

                return Err(e)
            }
        }

        #[cfg(not(feature = "read-tx-timeouts"))]
        let txn_manager = TxnManager::new(EnvPtr(env));

        #[cfg(feature = "read-tx-timeouts")]
        let txn_manager = {
            let mut txn_manager = TxnManager::new(EnvPtr(env));
            if let crate::MaxReadTransactionDuration::Set(duration) = self
                .max_read_transaction_duration
                .unwrap_or(read_transactions::MaxReadTransactionDuration::Set(
                    DEFAULT_MAX_READ_TRANSACTION_DURATION,
                ))
            {
                txn_manager = txn_manager.with_max_read_transaction_duration(duration);
            };
            txn_manager
        };

        let env = EnvironmentInner { env, txn_manager, env_kind: self.kind };

        Ok(Environment { inner: Arc::new(env) })
    }

    /// Configures how this environment will be opened.
    pub fn set_kind(&mut self, kind: EnvironmentKind) -> &mut Self {
        self.kind = kind;
        self
    }

    /// Opens the environment with mdbx WRITEMAP
    ///
    /// See also [EnvironmentKind]
    pub fn write_map(&mut self) -> &mut Self {
        self.set_kind(EnvironmentKind::WriteMap)
    }

    /// Sets the provided options in the environment.
    pub fn set_flags(&mut self, flags: EnvironmentFlags) -> &mut Self {
        self.flags = flags;
        self
    }

    /// Sets the maximum number of threads or reader slots for the environment.
    ///
    /// This defines the number of slots in the lock table that is used to track readers in the
    /// the environment. The default is 126. Starting a read-only transaction normally ties a lock
    /// table slot to the [Transaction] object until it or the [Environment] object is destroyed.
    pub fn set_max_readers(&mut self, max_readers: u64) -> &mut Self {
        self.max_readers = Some(max_readers);
        self
    }

    /// Sets the maximum number of named databases for the environment.
    ///
    /// This function is only needed if multiple databases will be used in the
    /// environment. Simpler applications that use the environment as a single
    /// unnamed database can ignore this option.
    ///
    /// Currently a moderate number of slots are cheap but a huge number gets
    /// expensive: 7-120 words per transaction, and every [Transaction::open_db()]
    /// does a linear search of the opened slots.
    pub fn set_max_dbs(&mut self, v: usize) -> &mut Self {
        self.max_dbs = Some(v as u64);
        self
    }

    pub fn set_rp_augment_limit(&mut self, v: u64) -> &mut Self {
        self.rp_augment_limit = Some(v);
        self
    }

    pub fn set_loose_limit(&mut self, v: u64) -> &mut Self {
        self.loose_limit = Some(v);
        self
    }

    pub fn set_dp_reserve_limit(&mut self, v: u64) -> &mut Self {
        self.dp_reserve_limit = Some(v);
        self
    }

    pub fn set_txn_dp_limit(&mut self, v: u64) -> &mut Self {
        self.txn_dp_limit = Some(v);
        self
    }

    pub fn set_spill_max_denominator(&mut self, v: u8) -> &mut Self {
        self.spill_max_denominator = Some(v.into());
        self
    }

    pub fn set_spill_min_denominator(&mut self, v: u8) -> &mut Self {
        self.spill_min_denominator = Some(v.into());
        self
    }

    /// Set all size-related parameters of environment, including page size and the min/max size of
    /// the memory map.
    pub fn set_geometry<R: RangeBounds<usize>>(&mut self, geometry: Geometry<R>) -> &mut Self {
        let convert_bound = |bound: Bound<&usize>| match bound {
            Bound::Included(v) | Bound::Excluded(v) => Some(*v),
            _ => None,
        };
        self.geometry = Some(Geometry {
            size: geometry.size.map(|range| {
                (convert_bound(range.start_bound()), convert_bound(range.end_bound()))
            }),
            growth_step: geometry.growth_step,
            shrink_threshold: geometry.shrink_threshold,
            page_size: geometry.page_size,
        });
        self
    }

    pub fn set_log_level(&mut self, log_level: ffi::MDBX_log_level_t) -> &mut Self {
        self.log_level = Some(log_level);
        self
    }

    /// Set the Handle-Slow-Readers callback. See [HandleSlowReadersCallback] for more information.
    #[cfg(not(windows))]
    pub fn set_handle_slow_readers(&mut self, hsr: HandleSlowReadersCallback) -> &mut Self {
        self.handle_slow_readers = Some(hsr);
        self
    }
}

#[cfg(feature = "read-tx-timeouts")]
pub(crate) mod read_transactions {
    use crate::EnvironmentBuilder;
    use std::time::Duration;

    /// The maximum duration of a read transaction.
    #[derive(Debug, Clone, Copy)]
    #[cfg(feature = "read-tx-timeouts")]
    pub enum MaxReadTransactionDuration {
        /// The maximum duration of a read transaction is unbounded.
        Unbounded,
        /// The maximum duration of a read transaction is set to the given duration.
        Set(Duration),
    }

    #[cfg(feature = "read-tx-timeouts")]
    impl MaxReadTransactionDuration {
        pub fn as_duration(&self) -> Option<Duration> {
            match self {
                MaxReadTransactionDuration::Unbounded => None,
                MaxReadTransactionDuration::Set(duration) => Some(*duration),
            }
        }
    }

    impl EnvironmentBuilder {
        /// Set the maximum time a read-only transaction can be open.
        pub fn set_max_read_transaction_duration(
            &mut self,
            max_read_transaction_duration: MaxReadTransactionDuration,
        ) -> &mut Self {
            self.max_read_transaction_duration = Some(max_read_transaction_duration);
            self
        }
    }
}

/// Creates an instance of `MDBX_hsr_func`.
///
/// Caution: this leaks the memory for callbacks, so they're alive throughout the program. It's
/// fine, because we also expect the database environment to be alive during this whole time.
#[cfg(not(windows))]
unsafe fn handle_slow_readers_callback(callback: HandleSlowReadersCallback) -> ffi::MDBX_hsr_func {
    // Move the callback function to heap and intentionally leak it, so it's not dropped and the
    // MDBX env can use it throughout the whole program.
    let callback = Box::leak(Box::new(callback));

    // Wrap the callback into an ffi binding. The callback is needed for a nicer UX with Rust types,
    // and without `env` and `txn` arguments that we don't want to expose to the user. Again,
    // move the closure to heap and leak.
    let hsr = Box::leak(Box::new(
        |_env: *const ffi::MDBX_env,
         _txn: *const ffi::MDBX_txn,
         pid: ffi::mdbx_pid_t,
         tid: ffi::mdbx_tid_t,
         laggard: u64,
         gap: ::libc::c_uint,
         space: usize,
         retry: ::libc::c_int|
         -> i32 {
            callback(pid as u32, tid as u32, laggard, gap as usize, space, retry as isize).into()
        },
    ));

    // Create a pointer to the C function from the Rust closure, and forcefully forget the original
    // closure.
    let closure = libffi::high::Closure8::new(hsr);
    let closure_ptr = *closure.code_ptr();
    std::mem::forget(closure);

    // Cast the closure to FFI `extern fn` type.
    Some(std::mem::transmute(closure_ptr))
}

#[cfg(test)]
mod tests {
    use crate::{Environment, Error, Geometry, HandleSlowReadersReturnCode, PageSize, WriteFlags};
    use std::{
        ops::RangeInclusive,
        sync::atomic::{AtomicBool, Ordering},
    };

    #[cfg(not(windows))]
    #[test]
    fn test_handle_slow_readers_callback() {
        static CALLED: AtomicBool = AtomicBool::new(false);

        let tempdir = tempfile::tempdir().unwrap();
        let env = Environment::builder()
            .set_geometry(Geometry::<RangeInclusive<usize>> {
                size: Some(0..=1024 * 1024), // Max 1MB, so we can hit the limit
                page_size: Some(PageSize::MinimalAcceptable), // To create as many pages as possible
                ..Default::default()
            })
            .set_handle_slow_readers(
                |_process_id: u32,
                 _thread_id: u32,
                 _read_txn_id: u64,
                 _gap: usize,
                 _space: usize,
                 _retry: isize| {
                    CALLED.store(true, Ordering::Relaxed);

                    HandleSlowReadersReturnCode::ProceedWithoutKillingReader
                },
            )
            .open(tempdir.path())
            .unwrap();

        // Insert some data in the database, so the read transaction can lock on the snapshot of it
        {
            let tx = env.begin_rw_txn().unwrap();
            let db = tx.open_db(None).unwrap();
            for i in 0usize..1_000 {
                tx.put(db.dbi(), i.to_le_bytes(), b"0", WriteFlags::empty()).unwrap()
            }
            tx.commit().unwrap();
        }

        // Create a read transaction
        let _tx_ro = env.begin_ro_txn().unwrap();

        // Change previously inserted data, so the read transaction would use the previous snapshot
        {
            let tx = env.begin_rw_txn().unwrap();
            let db = tx.open_db(None).unwrap();
            for i in 0usize..1_000 {
                tx.put(db.dbi(), i.to_le_bytes(), b"1", WriteFlags::empty()).unwrap();
            }
            tx.commit().unwrap();
        }

        // Insert more data in the database, so we hit the DB size limit error, and MDBX tries to
        // kick long-lived readers and delete their snapshots
        {
            let tx = env.begin_rw_txn().unwrap();
            let db = tx.open_db(None).unwrap();
            for i in 1_000usize..1_000_000 {
                match tx.put(db.dbi(), i.to_le_bytes(), b"0", WriteFlags::empty()) {
                    Ok(_) => continue,
                    Err(Error::MapFull) => break,
                    result @ Err(_) => result.unwrap(),
                }
            }
            tx.commit().unwrap();
        }

        // Expect the HSR to be called
        assert!(CALLED.load(Ordering::Relaxed));
    }
}
