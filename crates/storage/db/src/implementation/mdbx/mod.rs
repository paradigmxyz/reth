//! Module that interacts with MDBX.

use crate::{
    lockfile::StorageLock,
    metrics::DatabaseEnvMetrics,
    tables::{self, Tables},
    utils::default_page_size,
    DatabaseError, TableSet,
};
use eyre::Context;
use metrics::{gauge, Label};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    database_metrics::DatabaseMetrics,
    models::ClientVersion,
    transaction::{DbTx, DbTxMut},
};
use reth_libmdbx::{
    ffi, DatabaseFlags, Environment, EnvironmentFlags, Geometry, HandleSlowReadersReturnCode,
    MaxReadTransactionDuration, Mode, PageSize, SyncMode, RO, RW,
};
use reth_storage_errors::db::LogLevel;
use reth_tracing::tracing::error;
use std::{
    collections::HashMap,
    ops::{Deref, Range},
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tx::Tx;

pub mod cursor;
pub mod tx;

mod utils;

/// 1 KB in bytes
pub const KILOBYTE: usize = 1024;
/// 1 MB in bytes
pub const MEGABYTE: usize = KILOBYTE * 1024;
/// 1 GB in bytes
pub const GIGABYTE: usize = MEGABYTE * 1024;
/// 1 TB in bytes
pub const TERABYTE: usize = GIGABYTE * 1024;

/// MDBX allows up to 32767 readers (`MDBX_READERS_LIMIT`), but we limit it to slightly below that
const DEFAULT_MAX_READERS: u64 = 32_000;

/// Space that a read-only transaction can occupy until the warning is emitted.
/// See [`reth_libmdbx::EnvironmentBuilder::set_handle_slow_readers`] for more information.
const MAX_SAFE_READER_SPACE: usize = 10 * GIGABYTE;

/// Environment used when opening a MDBX environment. RO/RW.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseEnvKind {
    /// Read-only MDBX environment.
    RO,
    /// Read-write MDBX environment.
    RW,
}

impl DatabaseEnvKind {
    /// Returns `true` if the environment is read-write.
    pub const fn is_rw(&self) -> bool {
        matches!(self, Self::RW)
    }
}

/// Arguments for database initialization.
#[derive(Clone, Debug)]
pub struct DatabaseArguments {
    /// Client version that accesses the database.
    client_version: ClientVersion,
    /// Database geometry settings.
    geometry: Geometry<Range<usize>>,
    /// Database log level. If [None], the default value is used.
    log_level: Option<LogLevel>,
    /// Maximum duration of a read transaction. If [None], the default value is used.
    max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    /// Open environment in exclusive/monopolistic mode. If [None], the default value is used.
    ///
    /// This can be used as a replacement for `MDB_NOLOCK`, which don't supported by MDBX. In this
    /// way, you can get the minimal overhead, but with the correct multi-process and multi-thread
    /// locking.
    ///
    /// If `true` = open environment in exclusive/monopolistic mode or return `MDBX_BUSY` if
    /// environment already used by other process. The main feature of the exclusive mode is the
    /// ability to open the environment placed on a network share.
    ///
    /// If `false` = open environment in cooperative mode, i.e. for multi-process
    /// access/interaction/cooperation. The main requirements of the cooperative mode are:
    /// - Data files MUST be placed in the LOCAL file system, but NOT on a network share.
    /// - Environment MUST be opened only by LOCAL processes, but NOT over a network.
    /// - OS kernel (i.e. file system and memory mapping implementation) and all processes that
    ///   open the given environment MUST be running in the physically single RAM with
    ///   cache-coherency. The only exception for cache-consistency requirement is Linux on MIPS
    ///   architecture, but this case has not been tested for a long time).
    ///
    /// This flag affects only at environment opening but can't be changed after.
    exclusive: Option<bool>,
    /// MDBX allows up to 32767 readers (`MDBX_READERS_LIMIT`). This arg is to configure the max
    /// readers.
    max_readers: Option<u64>,
    /// Defines the synchronization strategy used by the MDBX database when writing data to disk.
    ///
    /// This determines how aggressively MDBX ensures data durability versus prioritizing
    /// performance. The available modes are:
    ///
    /// - [`SyncMode::Durable`]: Ensures all transactions are fully flushed to disk before they are
    ///   considered committed.   This provides the highest level of durability and crash safety
    ///   but may have a performance cost.
    /// - [`SyncMode::SafeNoSync`]: Skips certain fsync operations to improve write performance.
    ///   This mode still maintains database integrity but may lose the most recent transactions if
    ///   the system crashes unexpectedly.
    ///
    /// Choose `Durable` if consistency and crash safety are critical (e.g., production
    /// environments). Choose `SafeNoSync` if performance is more important and occasional data
    /// loss is acceptable (e.g., testing or ephemeral data).
    sync_mode: SyncMode,
}

impl Default for DatabaseArguments {
    fn default() -> Self {
        Self::new(ClientVersion::default())
    }
}

impl DatabaseArguments {
    /// Create new database arguments with given client version.
    pub fn new(client_version: ClientVersion) -> Self {
        Self {
            client_version,
            geometry: Geometry {
                size: Some(0..(8 * TERABYTE)),
                growth_step: Some(4 * GIGABYTE as isize),
                shrink_threshold: Some(0),
                page_size: Some(PageSize::Set(default_page_size())),
            },
            log_level: None,
            max_read_transaction_duration: None,
            exclusive: None,
            max_readers: None,
            sync_mode: SyncMode::Durable,
        }
    }

    /// Create database arguments suitable for testing.
    ///
    /// Uses a small geometry (64MB max, 4MB growth) to avoid exhausting the system's
    /// virtual memory map limit (`vm.max_map_count`) when many test databases are open
    /// concurrently.
    pub fn test() -> Self {
        Self {
            geometry: Geometry {
                size: Some(0..(64 * MEGABYTE)),
                growth_step: Some(4 * MEGABYTE as isize),
                shrink_threshold: Some(0),
                page_size: Some(PageSize::Set(default_page_size())),
            },
            max_read_transaction_duration: Some(MaxReadTransactionDuration::Unbounded),
            ..Self::new(ClientVersion::default())
        }
    }

    /// Sets the upper size limit of the db environment, the maximum database size in bytes.
    pub const fn with_geometry_max_size(mut self, max_size: Option<usize>) -> Self {
        if let Some(max_size) = max_size {
            self.geometry.size = Some(0..max_size);
        }
        self
    }

    /// Sets the database page size value.
    pub const fn with_geometry_page_size(mut self, page_size: Option<usize>) -> Self {
        if let Some(size) = page_size {
            self.geometry.page_size = Some(reth_libmdbx::PageSize::Set(size));
        }

        self
    }

    /// Sets the database sync mode.
    pub const fn with_sync_mode(mut self, sync_mode: Option<SyncMode>) -> Self {
        if let Some(sync_mode) = sync_mode {
            self.sync_mode = sync_mode;
        }

        self
    }

    /// Configures the database growth step in bytes.
    pub const fn with_growth_step(mut self, growth_step: Option<usize>) -> Self {
        if let Some(growth_step) = growth_step {
            self.geometry.growth_step = Some(growth_step as isize);
        }
        self
    }

    /// Set the log level.
    pub const fn with_log_level(mut self, log_level: Option<LogLevel>) -> Self {
        self.log_level = log_level;
        self
    }

    /// Set the maximum duration of a read transaction.
    pub const fn max_read_transaction_duration(
        &mut self,
        max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    ) {
        self.max_read_transaction_duration = max_read_transaction_duration;
    }

    /// Set the maximum duration of a read transaction.
    pub const fn with_max_read_transaction_duration(
        mut self,
        max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    ) -> Self {
        self.max_read_transaction_duration(max_read_transaction_duration);
        self
    }

    /// Set the mdbx exclusive flag.
    pub const fn with_exclusive(mut self, exclusive: Option<bool>) -> Self {
        self.exclusive = exclusive;
        self
    }

    /// Set `max_readers` flag.
    pub const fn with_max_readers(mut self, max_readers: Option<u64>) -> Self {
        self.max_readers = max_readers;
        self
    }

    /// Returns the client version if any.
    pub const fn client_version(&self) -> &ClientVersion {
        &self.client_version
    }
}

/// Wrapper for the libmdbx environment: [Environment]
#[derive(Debug, Clone)]
pub struct DatabaseEnv {
    /// Libmdbx-sys environment.
    inner: Environment,
    /// Opened DBIs for reuse.
    /// Important: Do not manually close these DBIs, like via `mdbx_dbi_close`.
    /// More generally, do not dynamically create, re-open, or drop tables at
    /// runtime. It's better to perform table creation and migration only once
    /// at startup.
    dbis: Arc<HashMap<&'static str, ffi::MDBX_dbi>>,
    /// Cache for metric handles. If `None`, metrics are not recorded.
    metrics: Option<Arc<DatabaseEnvMetrics>>,
    /// Write lock for when dealing with a read-write environment.
    _lock_file: Option<StorageLock>,
}

impl Database for DatabaseEnv {
    type TX = tx::Tx<RO>;
    type TXMut = tx::Tx<RW>;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        Tx::new(
            self.inner.begin_ro_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.dbis.clone(),
            self.metrics.clone(),
        )
        .map_err(|e| DatabaseError::InitTx(e.into()))
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        Tx::new(
            self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.dbis.clone(),
            self.metrics.clone(),
        )
        .map_err(|e| DatabaseError::InitTx(e.into()))
    }
}

impl DatabaseMetrics for DatabaseEnv {
    fn report_metrics(&self) {
        for (name, value, labels) in self.gauge_metrics() {
            gauge!(name, labels).set(value);
        }
    }

    fn gauge_metrics(&self) -> Vec<(&'static str, f64, Vec<Label>)> {
        let mut metrics = Vec::new();

        let _ = self
            .view(|tx| {
                for table in Tables::ALL.iter().map(Tables::name) {
                    let table_db =
                        tx.inner().open_db(Some(table)).wrap_err("Could not open db.")?;

                    let stats = tx
                        .inner()
                        .db_stat(table_db.dbi())
                        .wrap_err(format!("Could not find table: {table}"))?;

                    let page_size = stats.page_size() as usize;
                    let leaf_pages = stats.leaf_pages();
                    let branch_pages = stats.branch_pages();
                    let overflow_pages = stats.overflow_pages();
                    let num_pages = leaf_pages + branch_pages + overflow_pages;
                    let table_size = page_size * num_pages;
                    let entries = stats.entries();

                    metrics.push((
                        "db.table_size",
                        table_size as f64,
                        vec![Label::new("table", table)],
                    ));
                    metrics.push((
                        "db.table_pages",
                        leaf_pages as f64,
                        vec![Label::new("table", table), Label::new("type", "leaf")],
                    ));
                    metrics.push((
                        "db.table_pages",
                        branch_pages as f64,
                        vec![Label::new("table", table), Label::new("type", "branch")],
                    ));
                    metrics.push((
                        "db.table_pages",
                        overflow_pages as f64,
                        vec![Label::new("table", table), Label::new("type", "overflow")],
                    ));
                    metrics.push((
                        "db.table_entries",
                        entries as f64,
                        vec![Label::new("table", table)],
                    ));
                }

                Ok::<(), eyre::Report>(())
            })
            .map_err(|error| error!(%error, "Failed to read db table stats"));

        if let Ok(freelist) =
            self.freelist().map_err(|error| error!(%error, "Failed to read db.freelist"))
        {
            metrics.push(("db.freelist", freelist as f64, vec![]));
        }

        if let Ok(stat) = self.stat().map_err(|error| error!(%error, "Failed to read db.stat")) {
            metrics.push(("db.page_size", stat.page_size() as f64, vec![]));
        }

        metrics.push((
            "db.timed_out_not_aborted_transactions",
            self.timed_out_not_aborted_transactions() as f64,
            vec![],
        ));

        metrics
    }
}

impl DatabaseEnv {
    /// Opens the database at the specified path with the given `EnvKind`.
    ///
    /// It does not create the tables, for that call [`DatabaseEnv::create_tables`].
    pub fn open(
        path: &Path,
        kind: DatabaseEnvKind,
        args: DatabaseArguments,
    ) -> Result<Self, DatabaseError> {
        let _lock_file = if kind.is_rw() {
            StorageLock::try_acquire(path)
                .map_err(|err| DatabaseError::Other(err.to_string()))?
                .into()
        } else {
            None
        };

        let mut inner_env = Environment::builder();

        let mode = match kind {
            DatabaseEnvKind::RO => Mode::ReadOnly,
            DatabaseEnvKind::RW => {
                // enable writemap mode in RW mode
                inner_env.write_map();
                Mode::ReadWrite { sync_mode: args.sync_mode }
            }
        };

        // Note: We set max dbs to 256 here to allow for custom tables. This needs to be set on
        // environment creation.
        debug_assert!(Tables::ALL.len() <= 256, "number of tables exceed max dbs");
        inner_env.set_max_dbs(256);
        inner_env.set_geometry(args.geometry);

        fn is_current_process(id: u32) -> bool {
            #[cfg(unix)]
            {
                id == std::os::unix::process::parent_id() || id == std::process::id()
            }

            #[cfg(not(unix))]
            {
                id == std::process::id()
            }
        }

        extern "C" fn handle_slow_readers(
            _env: *const ffi::MDBX_env,
            _txn: *const ffi::MDBX_txn,
            process_id: ffi::mdbx_pid_t,
            thread_id: ffi::mdbx_tid_t,
            read_txn_id: u64,
            gap: std::ffi::c_uint,
            space: usize,
            retry: std::ffi::c_int,
        ) -> HandleSlowReadersReturnCode {
            if space > MAX_SAFE_READER_SPACE {
                let message = if is_current_process(process_id as u32) {
                    "Current process has a long-lived database transaction that grows the database file."
                } else {
                    "External process has a long-lived database transaction that grows the database file. \
                     Use shorter-lived read transactions or shut down the node."
                };
                reth_tracing::tracing::warn!(
                    target: "storage::db::mdbx",
                    ?process_id,
                    ?thread_id,
                    ?read_txn_id,
                    ?gap,
                    ?space,
                    ?retry,
                    "{message}"
                )
            }

            reth_libmdbx::HandleSlowReadersReturnCode::ProceedWithoutKillingReader
        }
        inner_env.set_handle_slow_readers(handle_slow_readers);

        inner_env.set_flags(EnvironmentFlags {
            mode,
            // We disable readahead because it improves performance for linear scans, but
            // worsens it for random access (which is our access pattern outside of sync)
            no_rdahead: true,
            coalesce: true,
            exclusive: args.exclusive.unwrap_or_default(),
            ..Default::default()
        });
        // Configure more readers
        inner_env.set_max_readers(args.max_readers.unwrap_or(DEFAULT_MAX_READERS));
        // This parameter sets the maximum size of the "reclaimed list", and the unit of measurement
        // is "pages". Reclaimed list is the list of freed pages that's populated during the
        // lifetime of DB transaction, and through which MDBX searches when it needs to insert new
        // record with overflow pages. The flow is roughly the following:
        // 0. We need to insert a record that requires N number of overflow pages (in consecutive
        //    sequence inside the DB file).
        // 1. Get some pages from the freelist, put them into the reclaimed list.
        // 2. Search through the reclaimed list for the sequence of size N.
        // 3. a. If found, return the sequence.
        // 3. b. If not found, repeat steps 1-3. If the reclaimed list size is larger than
        //    the `rp augment limit`, stop the search and allocate new pages at the end of the file:
        //    https://github.com/paradigmxyz/reth/blob/2a4c78759178f66e30c8976ec5d243b53102fc9a/crates/storage/libmdbx-rs/mdbx-sys/libmdbx/mdbx.c#L11479-L11480.
        //
        // Basically, this parameter controls for how long do we search through the freelist before
        // trying to allocate new pages. Smaller value will make MDBX to fallback to
        // allocation faster, higher value will force MDBX to search through the freelist
        // longer until the sequence of pages is found.
        //
        // The default value of this parameter is set depending on the DB size. The bigger the
        // database, the larger is `rp augment limit`.
        // https://github.com/paradigmxyz/reth/blob/2a4c78759178f66e30c8976ec5d243b53102fc9a/crates/storage/libmdbx-rs/mdbx-sys/libmdbx/mdbx.c#L10018-L10024.
        //
        // Previously, MDBX set this value as `256 * 1024` constant. Let's fallback to this,
        // because we want to prioritize freelist lookup speed over database growth.
        // https://github.com/paradigmxyz/reth/blob/fa2b9b685ed9787636d962f4366caf34a9186e66/crates/storage/libmdbx-rs/mdbx-sys/libmdbx/mdbx.c#L16017.
        inner_env.set_rp_augment_limit(256 * 1024);

        if let Some(log_level) = args.log_level {
            // Levels higher than [LogLevel::Notice] require libmdbx built with `MDBX_DEBUG` option.
            let is_log_level_available = if cfg!(debug_assertions) {
                true
            } else {
                matches!(
                    log_level,
                    LogLevel::Fatal | LogLevel::Error | LogLevel::Warn | LogLevel::Notice
                )
            };
            if is_log_level_available {
                inner_env.set_log_level(match log_level {
                    LogLevel::Fatal => 0,
                    LogLevel::Error => 1,
                    LogLevel::Warn => 2,
                    LogLevel::Notice => 3,
                    LogLevel::Verbose => 4,
                    LogLevel::Debug => 5,
                    LogLevel::Trace => 6,
                    LogLevel::Extra => 7,
                });
            } else {
                return Err(DatabaseError::LogLevelUnavailable(log_level))
            }
        }

        if let Some(max_read_transaction_duration) = args.max_read_transaction_duration {
            inner_env.set_max_read_transaction_duration(max_read_transaction_duration);
        }

        let env = Self {
            inner: inner_env.open(path).map_err(|e| DatabaseError::Open(e.into()))?,
            dbis: Arc::default(),
            metrics: None,
            _lock_file,
        };

        Ok(env)
    }

    /// Spawns a background thread that force-loads the entire database into the OS page cache.
    ///
    /// Uses `mdbx_env_warmup` with `FORCE | OOM_SAFE` to sequentially peek all allocated pages,
    /// loading them into memory without risking an OOM kill.
    pub fn start_warmup(&self) -> std::thread::JoinHandle<()> {
        let env = self.inner.clone();
        std::thread::Builder::new()
            .name("mdbx-warmup".to_string())
            .spawn(move || {
                let flags = reth_libmdbx::WarmupFlags::FORCE | reth_libmdbx::WarmupFlags::OOM_SAFE;

                reth_tracing::tracing::info!(
                    target: "storage::db::mdbx",
                    "Starting database warmup"
                );
                match env.warmup(flags, None) {
                    Ok(false) => {
                        reth_tracing::tracing::info!(
                            target: "storage::db::mdbx",
                            "Database warmup complete"
                        );
                    }
                    Ok(true) => {
                        reth_tracing::tracing::warn!(
                            target: "storage::db::mdbx",
                            "Database warmup timed out"
                        );
                    }
                    Err(err) => {
                        reth_tracing::tracing::warn!(
                            target: "storage::db::mdbx",
                            %err,
                            "Database warmup failed"
                        );
                    }
                }
            })
            .expect("failed to spawn mdbx-warmup thread")
    }

    /// Enables metrics on the database.
    pub fn with_metrics(mut self) -> Self {
        self.metrics = Some(DatabaseEnvMetrics::new().into());
        self
    }

    /// Creates all the tables defined in [`Tables`], if necessary.
    ///
    /// This keeps tracks of the created table handles and stores them for better efficiency.
    pub fn create_tables(&mut self) -> Result<(), DatabaseError> {
        self.create_and_track_tables_for::<Tables>()
    }

    /// Creates all the tables defined in the given [`TableSet`], if necessary.
    ///
    /// This keeps tracks of the created table handles and stores them for better efficiency.
    pub fn create_and_track_tables_for<TS: TableSet>(&mut self) -> Result<(), DatabaseError> {
        let handles = self._create_tables::<TS>()?;
        // Note: This is okay because self has mutable access here and `DatabaseEnv` must be Arc'ed
        // before it can be shared.
        let dbis = Arc::make_mut(&mut self.dbis);
        dbis.extend(handles);

        Ok(())
    }

    /// Creates all the tables defined in [`Tables`], if necessary.
    ///
    /// If this type is unique the created handle for the tables will be updated.
    ///
    /// This is recommended to be called during initialization to create and track additional tables
    /// after the default [`Self::create_tables`] are created.
    pub fn create_tables_for<TS: TableSet>(self: &mut Arc<Self>) -> Result<(), DatabaseError> {
        let handles = self._create_tables::<TS>()?;
        if let Some(db) = Arc::get_mut(self) {
            // Note: The db is unique and the dbis as well, and they can also be cloned.
            let dbis = Arc::make_mut(&mut db.dbis);
            dbis.extend(handles);
        }
        Ok(())
    }

    /// Creates the tables and returns the identifiers of the tables.
    fn _create_tables<TS: TableSet>(
        &self,
    ) -> Result<Vec<(&'static str, ffi::MDBX_dbi)>, DatabaseError> {
        let mut handles = Vec::new();
        let tx = self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?;

        for table in TS::tables() {
            let flags =
                if table.is_dupsort() { DatabaseFlags::DUP_SORT } else { DatabaseFlags::default() };

            let db = tx
                .create_db(Some(table.name()), flags)
                .map_err(|e| DatabaseError::CreateTable(e.into()))?;
            handles.push((table.name(), db.dbi()));
        }

        tx.commit().map_err(|e| DatabaseError::Commit(e.into()))?;
        Ok(handles)
    }

    /// Drops an orphaned table by name.
    ///
    /// This is used to clean up tables that are no longer defined in the schema but may still
    /// exist on disk from previous versions.
    ///
    /// Returns `Ok(true)` if the table existed and was dropped, `Ok(false)` if the table was not
    /// found.
    ///
    /// # Safety
    /// This permanently deletes the table and all its data. Only use for tables that are
    /// confirmed to be obsolete.
    pub fn drop_orphan_table(&self, name: &str) -> Result<bool, DatabaseError> {
        let tx = self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?;

        match tx.open_db(Some(name)) {
            Ok(db) => {
                // SAFETY: We just opened the db handle and will commit immediately after dropping.
                // No other cursors or handles exist for this table.
                unsafe {
                    tx.drop_db(db.dbi()).map_err(|e| DatabaseError::Delete(e.into()))?;
                }
                tx.commit().map_err(|e| DatabaseError::Commit(e.into()))?;
                Ok(true)
            }
            Err(reth_libmdbx::Error::NotFound) => Ok(false),
            Err(e) => Err(DatabaseError::Open(e.into())),
        }
    }

    /// Records version that accesses the database with write privileges.
    pub fn record_client_version(&self, version: ClientVersion) -> Result<(), DatabaseError> {
        if version.is_empty() {
            return Ok(())
        }

        let tx = self.tx_mut()?;
        let mut version_cursor = tx.cursor_write::<tables::VersionHistory>()?;

        let last_version = version_cursor.last()?.map(|(_, v)| v);
        if Some(&version) != last_version.as_ref() {
            version_cursor.upsert(
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                &version,
            )?;
            tx.commit()?;
        }

        Ok(())
    }
}

impl Deref for DatabaseEnv {
    type Target = Environment;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tables::{
            AccountsHistory, CanonicalHeaders, Headers, PlainAccountState, PlainStorageState,
        },
        test_utils::*,
        AccountChangeSets,
    };
    use alloy_consensus::Header;
    use alloy_primitives::{address, Address, B256, U256};
    use reth_db_api::{
        cursor::{DbDupCursorRO, DbDupCursorRW, ReverseWalker, Walker},
        models::{AccountBeforeTx, IntegerList, ShardedKey},
        table::{Encode, Table},
    };
    use reth_libmdbx::Error;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_storage_errors::db::{DatabaseWriteError, DatabaseWriteOperation};
    use std::str::FromStr;
    use tempfile::TempDir;

    /// Create database for testing. Returns the `TempDir` to prevent cleanup until test ends.
    fn create_test_db(kind: DatabaseEnvKind) -> (TempDir, DatabaseEnv) {
        let tempdir = tempfile::TempDir::new().expect(ERROR_TEMPDIR);
        let env = create_test_db_with_path(kind, tempdir.path());
        (tempdir, env)
    }

    /// Create database for testing with specified path
    fn create_test_db_with_path(kind: DatabaseEnvKind, path: &Path) -> DatabaseEnv {
        let mut env =
            DatabaseEnv::open(path, kind, DatabaseArguments::new(ClientVersion::default()))
                .expect(ERROR_DB_CREATION);
        env.create_tables().expect(ERROR_TABLE_CREATION);
        env
    }

    const ERROR_DB_CREATION: &str = "Not able to create the mdbx file.";
    const ERROR_PUT: &str = "Not able to insert value into table.";
    const ERROR_APPEND: &str = "Not able to append the value to the table.";
    const ERROR_UPSERT: &str = "Not able to upsert the value to the table.";
    const ERROR_GET: &str = "Not able to get value from table.";
    const ERROR_DEL: &str = "Not able to delete from table.";
    const ERROR_COMMIT: &str = "Not able to commit transaction.";
    const ERROR_RETURN_VALUE: &str = "Mismatching result.";
    const ERROR_INIT_TX: &str = "Failed to create a MDBX transaction.";
    const ERROR_ETH_ADDRESS: &str = "Invalid address.";

    #[test]
    fn db_creation() {
        let _tempdir = create_test_db(DatabaseEnvKind::RW);
    }

    #[test]
    fn db_drop_orphan_table() {
        let tempdir = tempfile::TempDir::new().expect(ERROR_TEMPDIR);
        let db = create_test_db_with_path(DatabaseEnvKind::RW, tempdir.path());

        // Create an orphan table by manually creating it
        let orphan_table_name = "OrphanTestTable";
        {
            let tx = db.inner.begin_rw_txn().expect(ERROR_INIT_TX);
            tx.create_db(Some(orphan_table_name), DatabaseFlags::empty())
                .expect("Failed to create orphan table");
            tx.commit().expect(ERROR_COMMIT);
        }

        // Verify the table exists by opening it
        {
            let tx = db.inner.begin_ro_txn().expect(ERROR_INIT_TX);
            assert!(tx.open_db(Some(orphan_table_name)).is_ok(), "Orphan table should exist");
        }

        // Drop the orphan table
        let result = db.drop_orphan_table(orphan_table_name);
        assert!(result.is_ok(), "drop_orphan_table should succeed");
        assert!(result.unwrap(), "drop_orphan_table should return true for existing table");

        // Verify the table no longer exists
        {
            let tx = db.inner.begin_ro_txn().expect(ERROR_INIT_TX);
            assert!(
                tx.open_db(Some(orphan_table_name)).is_err(),
                "Orphan table should no longer exist"
            );
        }

        // Dropping a non-existent table should return Ok(false)
        let result = db.drop_orphan_table("NonExistentTable");
        assert!(result.is_ok(), "drop_orphan_table should succeed for non-existent table");
        assert!(!result.unwrap(), "drop_orphan_table should return false for non-existent table");
    }

    #[test]
    fn db_manual_put_get() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);

        let value = Header::default();
        let key = 1u64;

        // PUT
        let tx = env.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<Headers>(key, value.clone()).expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // GET
        let tx = env.tx().expect(ERROR_INIT_TX);
        let result = tx.get::<Headers>(key).expect(ERROR_GET);
        assert_eq!(result.expect(ERROR_RETURN_VALUE), value);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_dup_cursor_delete_first() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

        let entry_0 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(0) };
        let entry_1 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };

        dup_cursor.upsert(Address::with_last_byte(1), &entry_0).expect(ERROR_UPSERT);
        dup_cursor.upsert(Address::with_last_byte(1), &entry_1).expect(ERROR_UPSERT);

        assert_eq!(
            dup_cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
            vec![(Address::with_last_byte(1), entry_0), (Address::with_last_byte(1), entry_1),]
        );

        let mut walker = dup_cursor.walk(None).unwrap();
        walker.delete_current().expect(ERROR_DEL);

        assert_eq!(walker.next().unwrap().unwrap(), (Address::with_last_byte(1), entry_1));

        // Check the tx view - it correctly holds entry_1
        assert_eq!(
            tx.cursor_dup_read::<PlainStorageState>()
                .unwrap()
                .walk(None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![
                (Address::with_last_byte(1), entry_1), // This is ok - we removed entry_0
            ]
        );

        // Check the remainder of walker
        assert!(walker.next().is_none());
    }

    #[test]
    fn db_cursor_walk() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);

        let value = Header::default();
        let key = 1u64;

        // PUT
        let tx = env.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<Headers>(key, value.clone()).expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // Cursor
        let tx = env.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<Headers>().unwrap();

        let first = cursor.first().unwrap();
        assert!(first.is_some(), "First should be our put");

        // Walk
        let walk = cursor.walk(Some(key)).unwrap();
        let first = walk.into_iter().next().unwrap().unwrap();
        assert_eq!(first.1, value, "First next should be put value");
    }

    #[test]
    fn db_cursor_walk_range() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT (0, 0), (1, 0), (2, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        // [1, 3)
        let mut walker = cursor.walk_range(1..3).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        assert!(walker.next().is_none());
        // next() returns None after walker is done
        assert!(walker.next().is_none());

        // [1, 2]
        let mut walker = cursor.walk_range(1..=2).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        // next() returns None after walker is done
        assert!(walker.next().is_none());

        // [1, ∞)
        let mut walker = cursor.walk_range(1..).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (3, B256::ZERO));
        // next() returns None after walker is done
        assert!(walker.next().is_none());

        // [2, 4)
        let mut walker = cursor.walk_range(2..4).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert!(walker.next().is_none());
        // next() returns None after walker is done
        assert!(walker.next().is_none());

        // (∞, 3)
        let mut walker = cursor.walk_range(..3).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        // next() returns None after walker is done
        assert!(walker.next().is_none());

        // (∞, ∞)
        let mut walker = cursor.walk_range(..).unwrap();
        assert_eq!(walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (2, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (3, B256::ZERO));
        // next() returns None after walker is done
        assert!(walker.next().is_none());
    }

    #[test]
    fn db_cursor_walk_range_on_dup_table() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        let address0 = Address::ZERO;
        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<AccountChangeSets>(0, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(0, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(0, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(1, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(1, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(1, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSets>(2, AccountBeforeTx { address: address0, info: None }) // <- should not be returned by the walker
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<AccountChangeSets>().unwrap();

        let entries = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 7);

        let mut walker = cursor.walk_range(0..=1).unwrap();
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (0, AccountBeforeTx { address: address0, info: None })
        );
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (0, AccountBeforeTx { address: address1, info: None })
        );
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (0, AccountBeforeTx { address: address2, info: None })
        );
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (1, AccountBeforeTx { address: address0, info: None })
        );
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (1, AccountBeforeTx { address: address1, info: None })
        );
        assert_eq!(
            walker.next().unwrap().unwrap(),
            (1, AccountBeforeTx { address: address2, info: None })
        );
        assert!(walker.next().is_none());
    }

    #[expect(clippy::reversed_empty_ranges)]
    #[test]
    fn db_cursor_walk_range_invalid() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT (0, 0), (1, 0), (2, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        // start bound greater than end bound
        let mut res = cursor.walk_range(3..1).unwrap();
        assert!(res.next().is_none());

        // start bound greater than end bound
        let mut res = cursor.walk_range(15..=2).unwrap();
        assert!(res.next().is_none());

        // returning nothing
        let mut walker = cursor.walk_range(1..1).unwrap();
        assert!(walker.next().is_none());
    }

    #[test]
    fn db_walker() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut walker = Walker::new(&mut cursor, None);

        assert_eq!(walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert!(walker.next().is_none());

        // transform to ReverseWalker
        let mut reverse_walker = walker.rev();
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());
    }

    #[test]
    fn db_reverse_walker() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut reverse_walker = ReverseWalker::new(&mut cursor, None);

        assert_eq!(reverse_walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());

        // transform to Walker
        let mut walker = reverse_walker.forward();
        assert_eq!(walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert!(walker.next().is_none());
    }

    #[test]
    fn db_walk_back() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT (0, 0), (1, 0), (3, 0)
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

        let mut reverse_walker = cursor.walk_back(Some(1)).unwrap();
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());

        let mut reverse_walker = cursor.walk_back(Some(2)).unwrap();
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());

        let mut reverse_walker = cursor.walk_back(Some(4)).unwrap();
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());

        let mut reverse_walker = cursor.walk_back(None).unwrap();
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (3, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (1, B256::ZERO));
        assert_eq!(reverse_walker.next().unwrap().unwrap(), (0, B256::ZERO));
        assert!(reverse_walker.next().is_none());
    }

    #[test]
    fn db_cursor_seek_exact_or_previous_key() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // Cursor
        let missing_key = 2;
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        assert!(cursor.current().unwrap().is_none());

        // Seek exact
        let exact = cursor.seek_exact(missing_key).unwrap();
        assert_eq!(exact, None);
        assert!(cursor.current().unwrap().is_none());
    }

    #[test]
    fn db_cursor_insert() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let key_to_insert = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

        // INSERT
        assert!(cursor.insert(key_to_insert, &B256::ZERO).is_ok());
        assert_eq!(cursor.current().unwrap(), Some((key_to_insert, B256::ZERO)));
        // INSERT (failure)
        assert!(matches!(
        cursor.insert(key_to_insert, &B256::ZERO).unwrap_err(),
        DatabaseError::Write(err) if *err == DatabaseWriteError {
            info: Error::KeyExist.into(),
            operation: DatabaseWriteOperation::CursorInsert,
            table_name: CanonicalHeaders::NAME,
            key: key_to_insert.encode().into(),
        }));
        assert_eq!(cursor.current().unwrap(), Some((key_to_insert, B256::ZERO)));

        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_insert_dup() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let key = Address::random();
        let subkey1 = B256::random();
        let subkey2 = B256::random();

        let entry1 = StorageEntry { key: subkey1, value: U256::ZERO };
        assert!(dup_cursor.insert(key, &entry1).is_ok());

        // Can't insert
        let entry2 = StorageEntry { key: subkey2, value: U256::ZERO };
        assert!(dup_cursor.insert(key, &entry2).is_err());
    }

    #[test]
    fn db_cursor_delete_current_non_existent() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let key1 = Address::with_last_byte(1);
        let key2 = Address::with_last_byte(2);
        let key3 = Address::with_last_byte(3);
        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();

        assert!(cursor.insert(key1, &Account::default()).is_ok());
        assert!(cursor.insert(key2, &Account::default()).is_ok());
        assert!(cursor.insert(key3, &Account::default()).is_ok());

        // Seek & delete key2
        cursor.seek_exact(key2).unwrap();
        assert!(cursor.delete_current().is_ok());
        assert!(cursor.seek_exact(key2).unwrap().is_none());

        // Seek & delete key2 again
        assert!(cursor.seek_exact(key2).unwrap().is_none());
        assert!(matches!(
            cursor.delete_current().unwrap_err(),
            DatabaseError::Delete(err) if err == reth_libmdbx::Error::NoData.into()));
        // Assert that key1 is still there
        assert_eq!(cursor.seek_exact(key1).unwrap(), Some((key1, Account::default())));
        // Assert that key3 is still there
        assert_eq!(cursor.seek_exact(key3).unwrap(), Some((key3, Account::default())));
    }

    #[test]
    fn db_cursor_insert_wherever_cursor_is() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        // PUT
        vec![0, 1, 3, 5, 7, 9]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

        // INSERT (cursor starts at last)
        cursor.last().unwrap();
        assert_eq!(cursor.current().unwrap(), Some((9, B256::ZERO)));

        for pos in (2..=8).step_by(2) {
            assert!(cursor.insert(pos, &B256::ZERO).is_ok());
            assert_eq!(cursor.current().unwrap(), Some((pos, B256::ZERO)));
        }
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 2, 3, 4]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND
        let key_to_append = 5;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
        assert!(cursor.append(key_to_append, &B256::ZERO).is_ok());
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append_failure() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        // PUT
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND
        let key_to_append = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
        assert!(matches!(
        cursor.append(key_to_append, &B256::ZERO).unwrap_err(),
        DatabaseError::Write(err) if *err == DatabaseWriteError {
            info: Error::KeyMismatch.into(),
            operation: DatabaseWriteOperation::CursorAppend,
            table_name: CanonicalHeaders::NAME,
            key: key_to_append.encode().into(),
        }));
        assert_eq!(cursor.current().unwrap(), Some((5, B256::ZERO))); // the end of table
        tx.commit().expect(ERROR_COMMIT);

        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_upsert() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();
        let key = Address::random();

        let account = Account::default();
        cursor.upsert(key, &account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key).unwrap(), Some((key, account)));

        let account = Account { nonce: 1, ..Default::default() };
        cursor.upsert(key, &account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key).unwrap(), Some((key, account)));

        let account = Account { nonce: 2, ..Default::default() };
        cursor.upsert(key, &account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key).unwrap(), Some((key, account)));

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let subkey = B256::random();

        let value = U256::from(1);
        let entry1 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, &entry1).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey).unwrap(), Some(entry1));

        let value = U256::from(2);
        let entry2 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, &entry2).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey).unwrap(), Some(entry1));
        assert_eq!(dup_cursor.next_dup_val().unwrap(), Some(entry2));
    }

    #[test]
    fn db_cursor_dupsort_append() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);

        let transition_id = 2;

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSets>().unwrap();
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|val| {
                cursor.append(
                    transition_id,
                    &AccountBeforeTx { address: Address::with_last_byte(val), info: None },
                )
            })
            .expect(ERROR_APPEND);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND DUP & APPEND
        let subkey_to_append = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSets>().unwrap();
        assert!(matches!(
        cursor
            .append_dup(
                transition_id,
                AccountBeforeTx {
                    address: Address::with_last_byte(subkey_to_append),
                    info: None
                }
            )
            .unwrap_err(),
        DatabaseError::Write(err) if *err == DatabaseWriteError {
            info: Error::KeyMismatch.into(),
            operation: DatabaseWriteOperation::CursorAppendDup,
            table_name: AccountChangeSets::NAME,
            key: transition_id.encode().into(),
        }));
        assert!(matches!(
            cursor
                .append(
                    transition_id - 1,
                    &AccountBeforeTx {
                        address: Address::with_last_byte(subkey_to_append),
                        info: None
                    }
                )
                .unwrap_err(),
            DatabaseError::Write(err) if *err == DatabaseWriteError {
                info: Error::KeyMismatch.into(),
                operation: DatabaseWriteOperation::CursorAppend,
                table_name: AccountChangeSets::NAME,
                key: (transition_id - 1).encode().into(),
            }
        ));
        assert!(cursor
            .append(
                transition_id,
                &AccountBeforeTx { address: Address::with_last_byte(subkey_to_append), info: None }
            )
            .is_ok());
    }

    #[test]
    fn db_closure_put_get() {
        let tempdir = TempDir::new().expect(ERROR_TEMPDIR);
        let path = tempdir.path();

        let value = Account {
            nonce: 18446744073709551615,
            bytecode_hash: Some(B256::random()),
            balance: U256::MAX,
        };
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        {
            let env = create_test_db_with_path(DatabaseEnvKind::RW, path);

            // PUT
            let result = env.update(|tx| {
                tx.put::<PlainAccountState>(key, value).expect(ERROR_PUT);
                200
            });
            assert_eq!(result.expect(ERROR_RETURN_VALUE), 200);
        }

        let env = DatabaseEnv::open(
            path,
            DatabaseEnvKind::RO,
            DatabaseArguments::new(ClientVersion::default()),
        )
        .expect(ERROR_DB_CREATION);

        // GET
        let result =
            env.view(|tx| tx.get::<PlainAccountState>(key).expect(ERROR_GET)).expect(ERROR_GET);

        assert_eq!(result, Some(value))
    }

    #[test]
    fn db_dup_sort() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // PUT (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key, value00).expect(ERROR_PUT)).unwrap();

        // PUT (2,2)
        let value22 = StorageEntry { key: B256::with_last_byte(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value22).expect(ERROR_PUT)).unwrap();

        // PUT (1,1)
        let value11 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value11).expect(ERROR_PUT)).unwrap();

        // Iterate with cursor
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            // Notice that value11 and value22 have been ordered in the DB.
            assert_eq!(Some(value00), cursor.next_dup_val().unwrap());
            assert_eq!(Some(value11), cursor.next_dup_val().unwrap());
            assert_eq!(Some(value22), cursor.next_dup_val().unwrap());
        }

        // Seek value with exact subkey
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let mut walker = cursor.walk_dup(Some(key), Some(B256::with_last_byte(1))).unwrap();
            assert_eq!(
                (key, value11),
                walker
                    .next()
                    .expect("element should exist.")
                    .expect("should be able to retrieve it.")
            );
        }
    }

    #[test]
    fn db_walk_dup_with_not_existing_key() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // PUT (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key, value00).expect(ERROR_PUT)).unwrap();

        // PUT (2,2)
        let value22 = StorageEntry { key: B256::with_last_byte(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value22).expect(ERROR_PUT)).unwrap();

        // PUT (1,1)
        let value11 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value11).expect(ERROR_PUT)).unwrap();

        // Try to walk_dup with not existing key should immediately return None
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let not_existing_key = Address::ZERO;
            let mut walker = cursor.walk_dup(Some(not_existing_key), None).unwrap();
            assert!(walker.next().is_none());
        }
    }

    #[test]
    fn db_iterate_over_all_dup_values() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);
        let key1 = Address::from_str("0x1111111111111111111111111111111111111111")
            .expect(ERROR_ETH_ADDRESS);
        let key2 = Address::from_str("0x2222222222222222222222222222222222222222")
            .expect(ERROR_ETH_ADDRESS);

        // PUT key1 (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key1, value00).expect(ERROR_PUT)).unwrap();

        // PUT key1 (1,1)
        let value11 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key1, value11).expect(ERROR_PUT)).unwrap();

        // PUT key2 (2,2)
        let value22 = StorageEntry { key: B256::with_last_byte(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key2, value22).expect(ERROR_PUT)).unwrap();

        // Iterate with walk_dup
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let mut walker = cursor.walk_dup(None, None).unwrap();

            // Notice that value11 and value22 have been ordered in the DB.
            assert_eq!((key1, value00), walker.next().unwrap().unwrap());
            assert_eq!((key1, value11), walker.next().unwrap().unwrap());
            // NOTE: Dup cursor does NOT iterates on all values but only on duplicated values of the
            // same key. assert_eq!(Ok(Some(value22.clone())), walker.next());
            assert!(walker.next().is_none());
        }

        // Iterate by using `walk`
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let first = cursor.first().unwrap().unwrap();
            let mut walker = cursor.walk(Some(first.0)).unwrap();
            assert_eq!((key1, value00), walker.next().unwrap().unwrap());
            assert_eq!((key1, value11), walker.next().unwrap().unwrap());
            assert_eq!((key2, value22), walker.next().unwrap().unwrap());
        }
    }

    #[test]
    fn dup_value_with_same_subkey() {
        let (_tempdir, env) = create_test_db(DatabaseEnvKind::RW);
        let key1 = Address::new([0x11; 20]);
        let key2 = Address::new([0x22; 20]);

        // PUT key1 (0,1)
        let value01 = StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key1, value01).expect(ERROR_PUT)).unwrap();

        // PUT key1 (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key1, value00).expect(ERROR_PUT)).unwrap();

        // PUT key2 (2,2)
        let value22 = StorageEntry { key: B256::with_last_byte(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key2, value22).expect(ERROR_PUT)).unwrap();

        // Iterate with walk
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let first = cursor.first().unwrap().unwrap();
            let mut walker = cursor.walk(Some(first.0)).unwrap();

            // NOTE: Both values are present
            assert_eq!((key1, value00), walker.next().unwrap().unwrap());
            assert_eq!((key1, value01), walker.next().unwrap().unwrap());
            assert_eq!((key2, value22), walker.next().unwrap().unwrap());
        }

        // seek_by_key_subkey
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            // NOTE: There are two values with same SubKey but only first one is shown
            assert_eq!(value00, cursor.seek_by_key_subkey(key1, value00.key).unwrap().unwrap());
            // key1 but value is greater than the one in the DB
            assert_eq!(None, cursor.seek_by_key_subkey(key1, value22.key).unwrap());
        }
    }

    #[test]
    fn db_sharded_key() {
        let (_tempdir, db) = create_test_db(DatabaseEnvKind::RW);
        let real_key = address!("0xa2c122be93b0074270ebee7f6b7292c7deb45047");

        let shards = 5;
        for i in 1..=shards {
            let key = ShardedKey::new(real_key, if i == shards { u64::MAX } else { i * 100 });
            let list = IntegerList::new_pre_sorted([i * 100u64]);

            db.update(|tx| tx.put::<AccountsHistory>(key.clone(), list.clone()).expect(""))
                .unwrap();
        }

        // Seek value with non existing key.
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountsHistory>().unwrap();

            // It will seek the one greater or equal to the query. Since we have `Address | 100`,
            // `Address | 200` in the database and we're querying `Address | 150` it will return us
            // `Address | 200`.
            let mut walker = cursor.walk(Some(ShardedKey::new(real_key, 150))).unwrap();
            let (key, list) = walker
                .next()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 200), key);
            let list200 = IntegerList::new_pre_sorted([200u64]);
            assert_eq!(list200, list);
        }
        // Seek greatest index
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountsHistory>().unwrap();

            // It will seek the MAX value of transition index and try to use prev to get first
            // biggers.
            let _unknown = cursor.seek_exact(ShardedKey::new(real_key, u64::MAX)).unwrap();
            let (key, list) = cursor
                .prev()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 400), key);
            let list400 = IntegerList::new_pre_sorted([400u64]);
            assert_eq!(list400, list);
        }
    }
}
