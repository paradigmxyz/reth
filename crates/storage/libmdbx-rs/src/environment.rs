use crate::{
    database::Database,
    error::{mdbx_result, Error, Result},
    flags::EnvironmentFlags,
    transaction::{RO, RW},
    Mode, Transaction, TransactionKind,
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
    sync::{
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
    thread::sleep,
    time::Duration,
};

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

    /// Returns true if the environment was opened in [Mode::ReadWrite] mode.
    #[inline]
    pub fn is_read_write(&self) -> bool {
        self.inner.txn_manager.is_some()
    }

    /// Returns true if the environment was opened in [Mode::ReadOnly] mode.
    #[inline]
    pub fn is_read_only(&self) -> bool {
        self.inner.txn_manager.is_none()
    }

    /// Returns the manager that handles transaction messages.
    ///
    /// Requires [Mode::ReadWrite] and returns None otherwise.
    #[inline]
    pub(crate) fn txn_manager(&self) -> Option<&SyncSender<TxnManagerMessage>> {
        self.inner.txn_manager.as_ref()
    }

    /// Returns the manager that handles transaction messages.
    ///
    /// Requires [Mode::ReadWrite] and returns None otherwise.
    #[inline]
    pub(crate) fn ensure_txn_manager(&self) -> Result<&SyncSender<TxnManagerMessage>> {
        self.txn_manager().ok_or(Error::WriteTransactionUnsupportedInReadOnlyMode)
    }

    /// Create a read-only transaction for use with the environment.
    #[inline]
    pub fn begin_ro_txn(&self) -> Result<Transaction<RO>> {
        Transaction::new(self.clone())
    }

    /// Create a read-write transaction for use with the environment. This method will block while
    /// there are any other read-write transactions open on the environment.
    pub fn begin_rw_txn(&self) -> Result<Transaction<RW>> {
        let sender = self.ensure_txn_manager()?;
        let txn = loop {
            let (tx, rx) = sync_channel(0);
            sender
                .send(TxnManagerMessage::Begin {
                    parent: TxnPtr(ptr::null_mut()),
                    flags: RW::OPEN_FLAGS,
                    sender: tx,
                })
                .unwrap();
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
    /// the sender half of the transaction manager channel
    ///
    /// Only set if the environment was opened in [Mode::ReadWrite] mode.
    txn_manager: Option<SyncSender<TxnManagerMessage>>,
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
    /// ([Mode::ReadOnly]).
    ///
    /// All data will be mapped into memory in the read-write mode [Mode::ReadWrite]. This offers a
    /// significant performance benefit, since the data will be modified directly in mapped
    /// memory and then flushed to disk by single system call, without any memory management
    /// nor copying.
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
pub(crate) struct TxnPtr(pub *mut ffi::MDBX_txn);
unsafe impl Send for TxnPtr {}
unsafe impl Sync for TxnPtr {}

#[derive(Copy, Clone, Debug)]
pub(crate) struct EnvPtr(pub *mut ffi::MDBX_env);
unsafe impl Send for EnvPtr {}
unsafe impl Sync for EnvPtr {}

pub(crate) enum TxnManagerMessage {
    Begin { parent: TxnPtr, flags: ffi::MDBX_txn_flags_t, sender: SyncSender<Result<TxnPtr>> },
    Abort { tx: TxnPtr, sender: SyncSender<Result<bool>> },
    Commit { tx: TxnPtr, sender: SyncSender<Result<bool>> },
}

/// Environment statistics.
///
/// Contains information about the size and layout of an MDBX environment or database.
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

        let mut env = EnvironmentInner { env, txn_manager: None, env_kind: self.kind };

        if let Mode::ReadWrite { .. } = self.flags.mode {
            let (tx, rx) = std::sync::mpsc::sync_channel(0);
            let e = EnvPtr(env.env);
            std::thread::spawn(move || loop {
                match rx.recv() {
                    Ok(msg) => match msg {
                        TxnManagerMessage::Begin { parent, flags, sender } => {
                            #[allow(clippy::redundant_locals)]
                            let e = e;
                            let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
                            sender
                                .send(
                                    mdbx_result(unsafe {
                                        ffi::mdbx_txn_begin_ex(
                                            e.0,
                                            parent.0,
                                            flags,
                                            &mut txn,
                                            ptr::null_mut(),
                                        )
                                    })
                                    .map(|_| TxnPtr(txn)),
                                )
                                .unwrap()
                        }
                        TxnManagerMessage::Abort { tx, sender } => {
                            sender.send(mdbx_result(unsafe { ffi::mdbx_txn_abort(tx.0) })).unwrap();
                        }
                        TxnManagerMessage::Commit { tx, sender } => {
                            sender
                                .send(mdbx_result(unsafe {
                                    ffi::mdbx_txn_commit_ex(tx.0, ptr::null_mut())
                                }))
                                .unwrap();
                        }
                    },
                    Err(_) => return,
                }
            });

            env.txn_manager = Some(tx);
        }

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
}
