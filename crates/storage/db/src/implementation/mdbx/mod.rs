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
    database_metrics::{DatabaseMetadata, DatabaseMetadataValue, DatabaseMetrics},
    models::ClientVersion,
    transaction::{DbTx, DbTxMut},
};
use reth_libmdbx::{
    ffi, DatabaseFlags, Environment, EnvironmentFlags, Geometry, HandleSlowReadersReturnCode,
    MaxReadTransactionDuration, Mode, PageSize, SyncMode, RO, RW,
};
use reth_storage_errors::db::LogLevel;
use reth_tracing::tracing::error;
use scalerize_client::ScalerizeClient;
use std::{
    ops::{Deref, Range},
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tx::Tx;

pub mod cursor;
pub mod scalerize_client;
pub mod tx;

/// 1 KB in bytes
pub const KILOBYTE: usize = 1024;
/// 1 MB in bytes
pub const MEGABYTE: usize = KILOBYTE * 1024;
/// 1 GB in bytes
pub const GIGABYTE: usize = MEGABYTE * 1024;
/// 1 TB in bytes
pub const TERABYTE: usize = GIGABYTE * 1024;

pub const TABLE_CODE_HASHED_ACCOUNTS: u8 = 0;
pub const TABLE_CODE_HASHED_STORAGES: u8 = 1;

pub const SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES: u8 = 40;
pub const SERIALIZED_HASHED_STORAGES_KEY_BYTES: u8 = 40;

/// MDBX allows up to 32767 readers (`MDBX_READERS_LIMIT`), but we limit it to slightly below that
const DEFAULT_MAX_READERS: u64 = 32_000;

/// Space that a read-only transaction can occupy until the warning is emitted.
/// See [`reth_libmdbx::EnvironmentBuilder::set_handle_slow_readers`] for more information.
const MAX_SAFE_READER_SPACE: usize = 10 * GIGABYTE;

/// Environment used when opening a MDBX environment. RO/RW.
#[derive(Debug)]
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
                size: Some(0..(4 * TERABYTE)),
                growth_step: Some(4 * GIGABYTE as isize),
                shrink_threshold: Some(0),
                page_size: Some(PageSize::Set(default_page_size())),
            },
            log_level: None,
            max_read_transaction_duration: None,
            exclusive: None,
        }
    }

    /// Sets the upper size limit of the db environment, the maximum database size in bytes.
    pub const fn with_geometry_max_size(mut self, max_size: Option<usize>) -> Self {
        if let Some(max_size) = max_size {
            self.geometry.size = Some(0..max_size);
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
    pub const fn with_max_read_transaction_duration(
        mut self,
        max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    ) -> Self {
        self.max_read_transaction_duration = max_read_transaction_duration;
        self
    }

    /// Set the mdbx exclusive flag.
    pub const fn with_exclusive(mut self, exclusive: Option<bool>) -> Self {
        self.exclusive = exclusive;
        self
    }

    /// Returns the client version if any.
    pub const fn client_version(&self) -> &ClientVersion {
        &self.client_version
    }
}

/// Wrapper for the libmdbx environment: [Environment]
#[derive(Debug)]
pub struct DatabaseEnv {
    /// Libmdbx-sys environment.
    inner: Environment,
    /// Cache for metric handles. If `None`, metrics are not recorded.
    metrics: Option<Arc<DatabaseEnvMetrics>>,
    /// Write lock for when dealing with a read-write environment.
    _lock_file: Option<StorageLock>,
}

impl Database for DatabaseEnv {
    type TX = tx::Tx<RO>;
    type TXMut = tx::Tx<RW>;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        let scalerize_client = Arc::new(std::sync::RwLock::new(
            ScalerizeClient::connect().map_err(DatabaseError::from)?,
        ));

        Tx::new_with_metrics(
            self.inner.begin_ro_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.metrics.clone(),
            scalerize_client.clone(),
        )
        .map_err(|e| DatabaseError::InitTx(e.into()))
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        let scalerize_client = ScalerizeClient::connect().map_err(DatabaseError::from)?;

        Tx::new_with_metrics(
            self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.metrics.clone(),
            Arc::new(std::sync::RwLock::new(scalerize_client)),
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
                    let table_db = tx.inner.open_db(Some(table)).wrap_err("Could not open db.")?;

                    let stats = tx
                        .inner
                        .db_stat(&table_db)
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

impl DatabaseMetadata for DatabaseEnv {
    fn metadata(&self) -> DatabaseMetadataValue {
        DatabaseMetadataValue::new(self.freelist().ok())
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
                Mode::ReadWrite { sync_mode: SyncMode::Durable }
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
        inner_env.set_max_readers(DEFAULT_MAX_READERS);
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
            metrics: None,
            _lock_file,
        };

        Ok(env)
    }

    /// Enables metrics on the database.
    pub fn with_metrics(mut self) -> Self {
        self.metrics = Some(DatabaseEnvMetrics::new().into());
        self
    }

    /// Creates all the tables defined in [`Tables`], if necessary.
    pub fn create_tables(&self) -> Result<(), DatabaseError> {
        self.create_tables_for::<Tables>()
    }

    /// Creates all the tables defined in the given [`TableSet`], if necessary.
    pub fn create_tables_for<TS: TableSet>(&self) -> Result<(), DatabaseError> {
        let tx = self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?;

        for table in TS::tables() {
            let flags =
                if table.is_dupsort() { DatabaseFlags::DUP_SORT } else { DatabaseFlags::default() };

            tx.create_db(Some(table.name()), flags)
                .map_err(|e| DatabaseError::CreateTable(e.into()))?;
        }

        tx.commit().map_err(|e| DatabaseError::Commit(e.into()))?;

        Ok(())
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
                version,
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
            AccountsHistory, CanonicalHeaders, HashedStorages, Headers, PlainAccountState,
            PlainStorageState,
        },
        test_utils::*,
        AccountChangeSets, HashedAccounts,
    };
    use alloy_consensus::Header;
    use alloy_primitives::{Address, B256, U160, U256};
    use bytes::BytesMut;
    use primitive_types::H256;
    use reth_db_api::{
        cursor::{DbDupCursorRO, DbDupCursorRW, ReverseWalker, Walker},
        models::{AccountBeforeTx, IntegerList, ShardedKey},
        table::{Encode, Table},
    };
    use reth_libmdbx::Error;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_storage_errors::db::{DatabaseWriteError, DatabaseWriteOperation};
    use rlp::{Decodable, Encodable, Rlp, RlpStream};
    use scalerize_client::{generate_unique_bytes, ScalerizeClient};
    use std::{ptr::null, str::FromStr, thread, time::Duration};
    use tempfile::TempDir;

    /// Create database for testing
    fn create_test_db(kind: DatabaseEnvKind) -> Arc<DatabaseEnv> {
        Arc::new(create_test_db_with_path(
            kind,
            &tempfile::TempDir::new().expect(ERROR_TEMPDIR).into_path(),
        ))
    }

    /// Create database for testing with specified path
    fn create_test_db_with_path(kind: DatabaseEnvKind, path: &Path) -> DatabaseEnv {
        let env = DatabaseEnv::open(path, kind, DatabaseArguments::new(ClientVersion::default()))
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
        create_test_db(DatabaseEnvKind::RW);
    }

    #[test]
    fn db_manual_put_get() {
        let env = create_test_db(DatabaseEnvKind::RW);

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
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

        let entry_0 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(0) };
        let entry_1 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };
        // dup_cursor.delete_current().expect(ERROR_DEL);
        println!("CURRENT: {:?}", dup_cursor.current());
        println!("DELETE CURRENT: {:?}", dup_cursor.delete_current());

        dup_cursor.upsert(Address::with_last_byte(1), entry_0).expect(ERROR_UPSERT);
        println!("CURRENT: {:?}", dup_cursor.current());
        // let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();

        println!("DELETE CURRENT: {:?}", dup_cursor.delete_current());
        println!("CURRENT: {:?}", dup_cursor.current());
        dup_cursor.upsert(Address::with_last_byte(1), entry_1).expect(ERROR_UPSERT);
        println!("CURRENT: {:?}", dup_cursor.current());

        assert_eq!(
            dup_cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>(),
            Ok(vec![(Address::with_last_byte(1), entry_0), (Address::with_last_byte(1), entry_1),])
        );

        let mut walker = dup_cursor.walk(None).unwrap();
        walker.delete_current().expect(ERROR_DEL);

        assert_eq!(walker.next(), Some(Ok((Address::with_last_byte(1), entry_1))));

        // Check the tx view - it correctly holds entry_1
        assert_eq!(
            tx.cursor_dup_read::<PlainStorageState>()
                .unwrap()
                .walk(None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec![
                (Address::with_last_byte(1), entry_1), // This is ok - we removed entry_0
            ])
        );

        // Check the remainder of walker
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_cursor_walk() {
        let env = create_test_db(DatabaseEnvKind::RW);

        // let value = Account::default();
        // let key = 1u64;

        let b256_var_from_bytes1 = B256::from([
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes2 = B256::from([
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes3 = B256::from([
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes4 = B256::from([
            3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes5 = B256::from([
            4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes6 = B256::from([
            1u8, 1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes7 = B256::from([
            1u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes8 = B256::from([
            1u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes9 = B256::from([
            2u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes10 = B256::from([
            4u8, 4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        // println!("for hashed storage");
        let subkey = B256::random();
        // let subkey: H256 = H256::zero();
        // println!("subkey {:?}", subkey);

        let value1 = U256::from(1);
        let value2 = U256::from(2);
        let value3 = U256::from(3);
        let value4 = U256::from(4);
        let entry1 = StorageEntry { key: b256_var_from_bytes2, value: value1 };
        let entry2 = StorageEntry { key: b256_var_from_bytes3, value: value2 };
        let entry3 = StorageEntry { key: b256_var_from_bytes4, value: value3 };
        let entry4 = StorageEntry { key: b256_var_from_bytes5, value: value4 };

        let entry5 = StorageEntry { key: b256_var_from_bytes6, value: value1 };
        let entry6 = StorageEntry { key: b256_var_from_bytes7, value: value2 };
        let entry7 = StorageEntry { key: b256_var_from_bytes8, value: value3 };
        let entry8 = StorageEntry { key: b256_var_from_bytes9, value: value4 };

        // PUT
        let tx = env.tx_mut().expect(ERROR_INIT_TX);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry1).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry2).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry3).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry4).expect(ERROR_PUT);

        tx.put::<HashedStorages>(b256_var_from_bytes5, entry5).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes5, entry6).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes5, entry7).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes5, entry8).expect(ERROR_PUT);

        tx.commit().expect(ERROR_COMMIT);
        let tx = env.tx().expect(ERROR_INIT_TX);

        let mut cursorr = tx.cursor_read::<HashedStorages>().unwrap();
        while let Some((key, value)) = cursorr.prev().unwrap() {
            println!("PREV Key: {}, Value: {:?}", key, value);
        }
        while let Some((key, value)) = cursorr.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }
        // Cursor
        let tx = env.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<HashedStorages>().unwrap();

        let first = cursor.first().unwrap();
        println!("FIRST: {:?}", first);
        // assert!(first.is_some(), "First should be our put");

        let seek_exact = cursor.seek_exact(b256_var_from_bytes1).unwrap();
        println!("SEEK EXACT: {:?}", seek_exact);

        let next = cursor.next().unwrap();
        println!("NEXT: {:?}", next);

        let seek_exact = cursor.seek_exact(b256_var_from_bytes5).unwrap();
        println!("SEEK EXACT: {:?}", seek_exact);

        let seek_exact = cursor.seek_exact(b256_var_from_bytes6).unwrap();
        println!("SEEK EXACT: {:?}", seek_exact);

        let seek_exact = cursor.seek_exact(b256_var_from_bytes7).unwrap();
        println!("SEEK EXACT: {:?}", seek_exact);

        let seek_exact = cursor.seek_exact(b256_var_from_bytes10).unwrap();
        println!("SEEK EXACT: {:?}", seek_exact);

        let seek = cursor.seek(b256_var_from_bytes1).unwrap();
        println!("SEEK: {:?}", seek);

        let seek = cursor.seek(b256_var_from_bytes5).unwrap();
        println!("SEEK: {:?}", seek);

        let seek = cursor.seek(b256_var_from_bytes6).unwrap();
        println!("SEEK: {:?}", seek);

        let seek = cursor.seek(b256_var_from_bytes7).unwrap();
        println!("SEEK: {:?}", seek);

        let seek = cursor.seek(b256_var_from_bytes10).unwrap();
        println!("SEEK: {:?}", seek);
        // Walk
        // let walk = cursor.walk(Some(key)).unwrap();
        // let first = walk.into_iter().next().unwrap().unwrap();
        // assert_eq!(first.1, value, "First next should be put value");
    }

    #[test]
    fn db_cursor_walk_range() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        assert_eq!(walker.next(), None);
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [1, 2]
        let mut walker = cursor.walk_range(1..=2).unwrap();
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [1, ∞)
        let mut walker = cursor.walk_range(1..).unwrap();
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((3, B256::ZERO))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // [2, 4)
        let mut walker = cursor.walk_range(2..4).unwrap();
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(walker.next(), None);
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // (∞, 3)
        let mut walker = cursor.walk_range(..3).unwrap();
        assert_eq!(walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);

        // (∞, ∞)
        let mut walker = cursor.walk_range(..).unwrap();
        assert_eq!(walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((2, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((3, B256::ZERO))));
        // next() returns None after walker is done
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_cursor_walk_range_on_dup_table() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address0, info: None }))));
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address1, info: None }))));
        assert_eq!(walker.next(), Some(Ok((0, AccountBeforeTx { address: address2, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address0, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address1, info: None }))));
        assert_eq!(walker.next(), Some(Ok((1, AccountBeforeTx { address: address2, info: None }))));
        assert_eq!(walker.next(), None);
    }

    #[allow(clippy::reversed_empty_ranges)]
    #[test]
    fn db_cursor_walk_range_invalid() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(res.next(), None);

        // start bound greater than end bound
        let mut res = cursor.walk_range(15..=2).unwrap();
        assert_eq!(res.next(), None);

        // returning nothing
        let mut walker = cursor.walk_range(1..1).unwrap();
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_walker() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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

        assert_eq!(walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(walker.next(), None);

        // transform to ReverseWalker
        let mut reverse_walker = walker.rev();
        assert_eq!(reverse_walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);
    }

    #[test]
    fn db_reverse_walker() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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

        assert_eq!(reverse_walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);

        // transform to Walker
        let mut walker = reverse_walker.forward();
        assert_eq!(walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(walker.next(), None);
    }

    #[test]
    fn db_walk_back() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(Some(2)).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(Some(4)).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);

        let mut reverse_walker = cursor.walk_back(None).unwrap();
        assert_eq!(reverse_walker.next(), Some(Ok((3, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((1, B256::ZERO))));
        assert_eq!(reverse_walker.next(), Some(Ok((0, B256::ZERO))));
        assert_eq!(reverse_walker.next(), None);
    }

    #[test]
    fn db_scalerize() {
        let zero = B256::ZERO;
        let b256_var_from_bytes1 = B256::from([
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes2 = B256::from([
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes3 = B256::from([
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes4 = B256::from([
            3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes5 = B256::from([
            4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes6 = B256::from([
            1u8, 1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes7 = B256::from([
            1u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes8 = B256::from([
            1u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes9 = B256::from([
            2u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes10 = B256::from([
            4u8, 4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);


        let account1 = Account {
            nonce: 1,
            balance: U256::from(1000),
            bytecode_hash: Some(U256::from(12345).into()),
        };

        let account2 = Account {
            nonce: 2,
            balance: U256::from(2000),
            bytecode_hash: Some(U256::from(12345).into()),
        };

        let account3 = Account {
            nonce: 3,
            balance: U256::from(3000),
            bytecode_hash: Some(U256::from(12345).into()),
        };

        let account4 = Account {
            nonce: 4,
            balance: U256::from(4000),
            bytecode_hash: Some(U256::from(42345).into()),
        };

        let subkey = B256::random();

        let value1 = U256::from(1);
        let value2 = U256::from(2);
        let value3 = U256::from(3);
        let value4 = U256::from(4);
        let entry1 = StorageEntry { key: b256_var_from_bytes2, value: value1 };
        let entry2 = StorageEntry { key: b256_var_from_bytes3, value: value2 };
        let entry3 = StorageEntry { key: b256_var_from_bytes4, value: value3 };
        let entry4 = StorageEntry { key: b256_var_from_bytes5, value: value4 };

        let entry5 = StorageEntry { key: b256_var_from_bytes6, value: value1 };
        let entry6 = StorageEntry { key: b256_var_from_bytes7, value: value2 };
        let entry7 = StorageEntry { key: b256_var_from_bytes8, value: value3 };
        let entry8 = StorageEntry { key: b256_var_from_bytes9, value: value4 };

        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut account_cursor = tx.cursor_write::<HashedAccounts>().unwrap();
        let mut storage_entry_cursor = tx.cursor_write::<HashedStorages>().unwrap();

        println!("FIRST ACCOUNT: {:?}", account_cursor.first());
        println!("FIRST STORAGEENTRY: {:?}", storage_entry_cursor.first());
        println!("GET ACCOUNT: {:?}", tx.get::<HashedAccounts>(b256_var_from_bytes1));
        println!("PUT ACCOUNT: {:?}", tx.put::<HashedAccounts>(b256_var_from_bytes1, account1));
        println!("PUT ACCOUNT: {:?}", tx.put::<HashedAccounts>(b256_var_from_bytes2, account2));
        println!("GET ACCOUNT: {:?}", tx.get::<HashedAccounts>(b256_var_from_bytes1));
        println!("FIRST ACCOUNT 1: {:?}", account_cursor.first());

        println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));
        println!("PUT STORAGEENTRY: {:?}", tx.put::<HashedStorages>(b256_var_from_bytes1, entry1));
        println!("PUT STORAGEENTRY: {:?}", tx.put::<HashedStorages>(b256_var_from_bytes1, entry2));
        println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));


        // println!("DELETE ACCOUNT: {:?}", tx.delete::<HashedAccounts>(b256_var_from_bytes2, None));
        // println!("GET ACCOUNT: {:?}", tx.get::<HashedAccounts>(b256_var_from_bytes1));
        // println!("DELETE ACCOUNT: {:?}", tx.delete::<HashedAccounts>(b256_var_from_bytes1, None));
        // println!("GET ACCOUNT: {:?}", tx.get::<HashedAccounts>(b256_var_from_bytes1));

        // println!("DELETE STORAGEENTRY: {:?}", tx.delete::<HashedStorages>(b256_var_from_bytes2, None));
        // println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));
        // println!("DELETE STORAGEENTRY: {:?}", tx.delete::<HashedStorages>(b256_var_from_bytes1, None));
        // println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));

        // println!("DELETE STORAGEENTRY: {:?}", tx.delete::<HashedStorages>(b256_var_from_bytes2, None));
        // println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));
        // println!("DELETE STORAGEENTRY: {:?}", tx.delete::<HashedStorages>(b256_var_from_bytes1, Some(entry1)));
        // println!("GET STORAGEENTRY: {:?}", tx.get::<HashedStorages>(b256_var_from_bytes1));

        println!("FIRST ACCOUNT 2: {:?}", account_cursor.first());
        println!("FIRST STORAGEENTRY: {:?}", storage_entry_cursor.first());

        println!("SEEK EXACT ACCOUNT 1: {:?}", account_cursor.seek_exact(b256_var_from_bytes1));
        println!("SEEK EXACT ACCOUNT 2: {:?}", account_cursor.seek_exact(zero));

        println!("SEEK EXACT STORAGEENTRY 1: {:?}", storage_entry_cursor.seek_exact(b256_var_from_bytes1));
        println!("SEEK EXACT STORAGEENTRY 2: {:?}", storage_entry_cursor.seek_exact(zero));

        println!("SEEK ACCOUNT: {:?}", account_cursor.seek(zero));
        println!("SEEK STORAGEENTRY: {:?}", storage_entry_cursor.seek(zero));

        // println!("SEEK ACCOUNT: {:?}", account_cursor.seek(b256_var_from_bytes9));
        // println!("SEEK STORAGEENTRY: {:?}", storage_entry_cursor.seek(b256_var_from_bytes9));

        println!("NEXT ACCOUNT: {:?}", account_cursor.next());
        println!("NEXT STORAGEENTRY: {:?}", storage_entry_cursor.next());
        println!("NEXT ACCOUNT: {:?}", account_cursor.next());
        println!("NEXT STORAGEENTRY: {:?}", storage_entry_cursor.next());

        println!("PREV ACCOUNT: {:?}", account_cursor.prev());
        println!("PREV STORAGEENTRY: {:?}", storage_entry_cursor.prev());
        println!("PREV ACCOUNT: {:?}", account_cursor.prev());
        println!("PREV STORAGEENTRY: {:?}", storage_entry_cursor.prev());
        
        println!("LAST ACCOUNT: {:?}", account_cursor.last());
        println!("LAST STORAGEENTRY: {:?}", storage_entry_cursor.last());

        println!("CURRENT ACCOUNT: {:?}", account_cursor.current());
        println!("CURRENT STORAGEENTRY: {:?}", storage_entry_cursor.current());

        println!("UPSERT ACCOUNT: {:?}", account_cursor.upsert(b256_var_from_bytes9, account3));
        println!("UPSERT STORAGEENTRY: {:?}", storage_entry_cursor.upsert(b256_var_from_bytes9, entry3));

        println!("CURRENT ACCOUNT: {:?}", account_cursor.current());
        println!("CURRENT STORAGEENTRY: {:?}", storage_entry_cursor.current());

        println!("INSERT ACCOUNT: {:?}", account_cursor.insert(b256_var_from_bytes9, account3));
        println!("INSERT STORAGEENTRY: {:?}", storage_entry_cursor.insert(b256_var_from_bytes9, entry3));

        println!("INSERT ACCOUNT: {:?}", account_cursor.insert(b256_var_from_bytes8, account4));
        println!("INSERT STORAGEENTRY: {:?}", storage_entry_cursor.insert(b256_var_from_bytes8, entry4));

        println!("CURRENT ACCOUNT: {:?}", account_cursor.current());
        println!("CURRENT STORAGEENTRY: {:?}", storage_entry_cursor.current());

        println!("APPEND ACCOUNT: {:?}", account_cursor.append(b256_var_from_bytes7, account1));
        println!("APPEND STORAGEENTRY: {:?}", storage_entry_cursor.append(b256_var_from_bytes7, entry1));

        tx.commit().expect(ERROR_COMMIT);
        println!("APPEND ACCOUNT: {:?}", account_cursor.append(b256_var_from_bytes9, account1));
        println!("APPEND STORAGEENTRY: {:?}", storage_entry_cursor.append(b256_var_from_bytes9, entry1));

        println!("DELETCURRENT ACCOUNT: {:?}", account_cursor.delete_current());
        println!("DELETCURRENT STORAGEENTRY: {:?}", storage_entry_cursor.delete_current());

        println!("CURRENT ACCOUNT: {:?}", account_cursor.current());
        println!("CURRENT STORAGEENTRY: {:?}", storage_entry_cursor.current());

        println!("DELETCURRENT ACCOUNT: {:?}", account_cursor.delete_current());
        println!("DELETCURRENT STORAGEENTRY: {:?}", storage_entry_cursor.delete_current());

    }

    #[test]
    fn db_cursor_seek_exact_or_previous_key() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(cursor.current(), Ok(None));
        assert_eq!(cursor.current(), Ok(None));
        while let Some((key, value)) = cursor.prev().unwrap() {
            println!("PREV Key: {}, Value: {:?}", key, value);
        }
        while let Some((key, value)) = cursor.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }

        // Seek exact
        let exact = cursor.seek_exact(missing_key).unwrap();
        assert_eq!(exact, None);
        assert_eq!(cursor.current(), Ok(Some((missing_key + 1, B256::ZERO))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 1, B256::ZERO))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 2, B256::ZERO))));
    }

    // #[test]
    // fn db_cursor_insert() -> Result<(), reth_db_api::DatabaseError> {
    //     // let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

    //     // // PUT
    //     // let tx = db.tx_mut().expect(ERROR_INIT_TX);
    //     // vec![0, 1, 3, 4, 5]
    //     //     .into_iter()
    //     //     .try_for_each(|key| tx.put::<CanonicalHeaders>(key, B256::ZERO))
    //     //     .expect(ERROR_PUT);
    //     // tx.commit().expect(ERROR_COMMIT);

    //     // let key_to_insert = 2;
    //     // let tx = db.tx_mut().expect(ERROR_INIT_TX);
    //     // let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

    //     // // INSERT
    //     // assert_eq!(cursor.insert(key_to_insert, B256::ZERO), Ok(()));
    //     // assert_eq!(cursor.current(), Ok(Some((key_to_insert, B256::ZERO))));

    //     // // INSERT (failure)
    //     // assert_eq!(
    //     //     cursor.insert(key_to_insert, B256::ZERO),
    //     //     Err(DatabaseWriteError {
    //     //         info: Error::KeyExist.into(),
    //     //         operation: DatabaseWriteOperation::CursorInsert,
    //     //         table_name: CanonicalHeaders::NAME,
    //     //         key: key_to_insert.encode().into(),
    //     //     }
    //     //     .into())
    //     // );
    //     // assert_eq!(cursor.current(), Ok(Some((key_to_insert, B256::ZERO))));

    //     // tx.commit().expect(ERROR_COMMIT);

    //     // // Confirm the result
    //     // let tx = db.tx().expect(ERROR_INIT_TX);
    //     // let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
    //     // let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
    //     // assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
    //     // tx.commit().expect(ERROR_COMMIT);

    //     // let key1 = B256::random();
    //     // let key2 = B256::random();
    //     // let key3 = B256::random();
    //     let key1 = B256::from([
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let key2 = B256::from([
    //         1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let key3 = B256::from([
    //         2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let key4 = B256::from([
    //         1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);
    //     println!("KEY 1: {:?}", key1.as_slice());
    //     println!("KEY 2: {:?}", key2.as_slice());
    //     println!("KEY 3: {:?}", key3.as_slice());

    //     let value1 = Account {
    //         nonce: 1,
    //         balance: U256::from(1000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let value2 = Account {
    //         nonce: 2,
    //         balance: U256::from(2000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let value3 = Account {
    //         nonce: 3,
    //         balance: U256::from(3000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let rlp_encoded1 = rlp::encode(&value1);
    //     println!("RLP Encoded Value 1: {:?}", rlp_encoded1.to_vec());
    //     let decoded_account: Account = rlp::decode(&rlp_encoded1).expect("Failed to decode RLP");
    //     println!("Decoded Account 1: {:?}", decoded_account);

    //     let rlp_encoded2 = rlp::encode(&value2);
    //     println!("RLP Encoded Value 2: {:?}", rlp_encoded2.to_vec());
    //     let decoded_account: Account = rlp::decode(&rlp_encoded2).expect("Failed to decode RLP");
    //     println!("Decoded Account 2: {:?}", decoded_account);

    //     let rlp_encoded3 = rlp::encode(&value3);
    //     println!("RLP Encoded Value 2: {:?}", rlp_encoded2.to_vec());
    //     let decoded_account: Account = rlp::decode(&rlp_encoded3).expect("Failed to decode RLP");
    //     println!("Decoded Account 2: {:?}", decoded_account);

    //     let mut scalerize_client = ScalerizeClient::connect()
    //     .map_err(DatabaseError::from)?;

    //     // PUT FOR HASHEDACCOUNTS
    //     scalerize_client.put(0, key1.as_slice(),None, &rlp_encoded1)
    //     .expect("PUT REQUEST FAILED");

    //     // println!("BYTES TO BE DECODED:{:?}", &put_response);
    //     // let rlp = Rlp::new(&put_response);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     // println!("PUT Account:{:?}", decoded_account);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     scalerize_client.put(0, key2.as_slice(),None, &rlp_encoded2)
    //     .expect("Failed to get put response");

    //     // println!("BYTES TO BE DECODED:{:?}", &put_response);
    //     // let rlp = Rlp::new(&put_response);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     // println!("PUT Account:{:?}", decoded_account);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     scalerize_client.put(0, key3.as_slice(),None, &rlp_encoded3)
    //     .expect("PUT REQUEST FAILED");

    //     // println!("BYTES TO BE DECODED:{:?}", &put_response);
    //     // let rlp = Rlp::new(&put_response);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     // println!("PUT Account:{:?}", decoded_account);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }
    //     // Try put request for table which does not exists
    //     // let put_response = scalerize_client.put(2, key2.as_slice(),&[], &rlp_encoded)
    //     // .expect("Failed to get put response");

    //     // println!("BYTES TO BE DECODED:{:?}", &put_response);
    //     // let rlp = Rlp::new(&put_response);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     // println!("PUT Account:{:?}", decoded_account);

    //     // GET FOR HASHEDACCOUNTS
    //     let get_response = scalerize_client.get(0, key1.as_slice()).
    //     expect("Failed to put response");

    //     println!("BYTES TO BE DECODED:{:?}", &get_response);
    //     let rlp = Rlp::new(&get_response);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("GET Account 1:{:?}", decoded_account);

    //     let get_response = scalerize_client.get(0, key2.as_slice()).
    //     expect("Failed to put response");

    //     println!("BYTES TO BE DECODED:{:?}", &get_response);
    //     let rlp = Rlp::new(&get_response);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("GET Account 2:{:?}", decoded_account);

    //     let b256_var_from_bytes1 = B256::from([
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes2 = B256::from([
    //         1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes3 = B256::from([
    //         2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes4 = B256::from([
    //         3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes5 = B256::from([
    //         4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes6 = B256::from([
    //         1u8, 1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes7 = B256::from([
    //         1u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes8 = B256::from([
    //         1u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes9 = B256::from([
    //         2u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes10 = B256::from([
    //         4u8, 4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
    //     ]);

    //     // println!("for hashed storage");
    //     let subkey = B256::random();
    //     // let subkey: H256 = H256::zero();
    //     // println!("subkey {:?}", subkey);

    //     let value1 = U256::from(1);
    //     let value2 = U256::from(2);
    //     let value3 = U256::from(3);
    //     let value4 = U256::from(4);
    //     let entry1 = StorageEntry { key: b256_var_from_bytes2, value: value1 };
    //     let entry2 = StorageEntry { key: b256_var_from_bytes3, value: value2 };
    //     let entry3 = StorageEntry { key: b256_var_from_bytes4, value: value3 };
    //     let entry4 = StorageEntry { key: b256_var_from_bytes5, value: value4 };

    //     let entry5 = StorageEntry { key: b256_var_from_bytes6, value: value1 };
    //     let entry6 = StorageEntry { key: b256_var_from_bytes7, value: value2 };
    //     let entry7 = StorageEntry { key: b256_var_from_bytes8, value: value3 };
    //     let entry8 = StorageEntry { key: b256_var_from_bytes9, value: value4 };

    //     // // let storage_entry = StorageEntry {
    //     // //     key: B256::random(),
    //     // //     value: U256::from(1),
    //     // // };

    //     scalerize_client.put(1, key1.as_slice(), Some(entry1.key.as_slice()),
    // &entry1.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     // let rlp_encoded = rlp::encode(&entry2);
    //     // let decoded_storage_entry: StorageEntry = rlp::decode(&rlp_encoded).expect("Failed to
    // decode RLP");     // println!("Decoded STORAGE ENTRY: {:?}", decoded_storage_entry);
    //     scalerize_client.put(1, key1.as_slice(), Some(entry2.key.as_slice()),
    // &entry2.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key1.as_slice(), Some(entry3.key.as_slice()),
    // &entry3.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key1.as_slice(), Some(entry4.key.as_slice()),
    // &entry4.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key3.as_slice(), Some(entry5.key.as_slice()),
    // &entry1.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key3.as_slice(), Some(entry6.key.as_slice()),
    // &entry2.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key3.as_slice(), Some(entry7.key.as_slice()),
    // &entry3.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     scalerize_client.put(1, key3.as_slice(), Some(entry8.key.as_slice()),
    // &entry8.value.to_be_bytes::<32>())     .expect("Failed to get put response");
    //     // println!("PUT RESPONSE: {:?}", put_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let get_response = scalerize_client.get(1, key1.as_slice()).
    //     expect("Failed to get response");

    //     let storage_entry = StorageEntry{
    //         key: B256::from_slice(&get_response[..32]),
    //         value:U256::from_be_slice(&get_response[32..]),
    //     };

    //     println!("GET Storage Entry:{:?}", storage_entry);

    //     // println!("BYTES TO BE DECODED:{:?}", &get_response);
    //     // let rlp = Rlp::new(&get_response);
    //     // let decoded_storage_entry = StorageEntry::decode(&rlp).expect("Failed to decode");
    //     // println!("GET Storage Entry:{:?}", decoded_storage_entry);

    //     // this will delete all entries for HashedStorages for account address key1
    //     // match scalerize_client.delete(1, key1.as_slice(), &[]) {
    //     //     Ok(_) => {
    //     //         println!("DELETE REQUEST SUCCESSFUL");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Delete request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     // this will delete all entries in the substore created for an account
    //     // match scalerize_client.delete(0, key1.as_slice(), &[]) {
    //     //     Ok(_) => {
    //     //         println!("DELETE REQUEST SUCCESSFUL");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Delete request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     // let rlp_encoded = rlp::encode(&entry1);

    //     // this will delete only entry for table HashedStorages with account address == key1 and
    // subkey == entry1     // match scalerize_client.delete(1, key1.as_slice(),
    // Some(entry1.key.as_slice())) {     //     Ok(_) => {
    //     //         println!("DELETE REQUEST SUCCESSFUL");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Delete request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let get_response = scalerize_client.get(1, key1.as_slice()).
    //     expect("Failed to get response");

    //     let storage_entry = StorageEntry{
    //         key: B256::from_slice(&get_response[..32]),
    //         value:U256::from_be_slice(&get_response[32..]),
    //     };

    //     println!("GET Storage Entry:{:?}", storage_entry);

    //     // this will delete all the entries for key
    //     // match scalerize_client.delete(1, key1.as_slice(), None) {
    //     //     Ok(_) => {
    //     //         println!("DELETE REQUEST SUCCESSFUL");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Delete request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     // match scalerize_client.write() {
    //     //     Ok(_) => {
    //     //         println!("WRITTEN TO DB");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Write request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     let get_response = scalerize_client.get(1, key1.as_slice()).
    //     expect("Failed to get response");

    //     let storage_entry = StorageEntry{
    //         key: B256::from_slice(&get_response[..32]),
    //         value:U256::from_be_slice(&get_response[32..]),
    //     };

    //     println!("GET Storage Entry:{:?}", storage_entry);

    //     // this will delete all the entries for key
    //     // match scalerize_client.delete(1, key1.as_slice(), None) {
    //     //     Ok(_) => {
    //     //         println!("DELETE REQUEST SUCCESSFUL");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Delete request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     // match scalerize_client.write() {
    //     //     Ok(_) => {
    //     //         println!("WRITTEN TO DB");
    //     //     }
    //     //     Err(e) => {
    //     //         println!("Write request failed with error: {:?}", e);
    //     //     }
    //     // }

    //     // this should fail since we deleted all entries for that key
    //     let get_response = scalerize_client.get(1, key1.as_slice()).
    //     expect("Failed to get response");

    //     let storage_entry = StorageEntry{
    //         key: B256::from_slice(&get_response[..32]),
    //         value:U256::from_be_slice(&get_response[32..]),
    //     };

    //     println!("GET Storage Entry:{:?}", storage_entry);

    //     let cursor_id_1 = generate_unique_bytes();
    //     let cursor_id_2 = generate_unique_bytes();
    //     let cursor_id_3 = generate_unique_bytes();
    //     let cursor_id_4 = generate_unique_bytes();
    //     let cursor_id_5 = generate_unique_bytes();

    //     let cursor_id_6 = generate_unique_bytes();
    //     let cursor_id_7 = generate_unique_bytes();
    //     let cursor_id_8 = generate_unique_bytes();
    //     let cursor_id_9 = generate_unique_bytes();
    //     let cursor_id_10 = generate_unique_bytes();
    //     let cursor_id_11 = generate_unique_bytes();
    //     let cursor_id_12 = generate_unique_bytes();
    //     let cursor_id_13 = generate_unique_bytes();
    //     let cursor_id_14 = generate_unique_bytes();
    //     let cursor_id_15 = generate_unique_bytes();
    //     let cursor_id_16 = generate_unique_bytes();
    //     let cursor_id_17 = generate_unique_bytes();
    //     let cursor_id_18 = generate_unique_bytes();
    //     let cursor_id_19 = generate_unique_bytes();
    //     let cursor_id_20 = generate_unique_bytes();
    //     let cursor_id_21 = generate_unique_bytes();
    //     let cursor_id_22 = generate_unique_bytes();
    //     let cursor_id_23 = generate_unique_bytes();
    //     let cursor_id_24 = generate_unique_bytes();
    //     let cursor_id_25 = generate_unique_bytes();

    //     let first_response = scalerize_client.first(0, cursor_id_1.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = first_response.0;
    //     let encoded_account = first_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("FIRST ACCOUNT KEY: {:?}", key);
    //     println!("FIRST ACCOUNT :{:?}", decoded_account);

    //     let seek_exact_response = scalerize_client.seek_exact(0, cursor_id_2.to_vec(),
    // key2.as_slice()).     expect("Failed to get seek_exact for accounts");

    //     if !seek_exact_response.is_empty() {
    //         let rlp = Rlp::new(&seek_exact_response);
    //         let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //         println!("SEEK_EXACT ACCOUNT KEY: {:?}", key2);
    //         println!("SEEK_EXACT ACCOUNT :{:?}", decoded_account);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_exact_response = scalerize_client.seek_exact(0, cursor_id_2.to_vec(),
    // key2.as_slice()).     expect("Failed to get seek_exact for accounts");

    //     if !seek_exact_response.is_empty() {
    //         let rlp = Rlp::new(&seek_exact_response);
    //         let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //         println!("SEEK_EXACT ACCOUNT KEY: {:?}", key2);
    //         println!("SEEK_EXACT ACCOUNT :{:?}", decoded_account);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_exact_response = scalerize_client.seek_exact(0, cursor_id_2.to_vec(),
    // key2.as_slice()).     expect("Failed to get seek_exact for accounts");
    //     if !seek_exact_response.is_empty() {
    //         let rlp = Rlp::new(&seek_exact_response);
    //         let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //         println!("SEEK_EXACT ACCOUNT KEY: {:?}", key2);
    //         println!("SEEK_EXACT ACCOUNT :{:?}", decoded_account);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_exact_response = scalerize_client.seek_exact(0, cursor_id_2.to_vec(),
    // key4.as_slice()).     expect("Failed to get seek_exact for accounts");

    //     if !seek_exact_response.is_empty() {
    //         let rlp = Rlp::new(&seek_exact_response);
    //         let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //         println!("SEEK_EXACT ACCOUNT KEY: {:?}", key4);
    //         println!("SEEK_EXACT ACCOUNT :{:?}", decoded_account);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_response = scalerize_client.seek(0, cursor_id_2.to_vec(), key2.as_slice()).
    //     expect("Failed to get seek for accounts");
    //     let key = seek_response.0;
    //     let encoded_account = seek_response.1;
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("SEEK ACCOUNT KEY: {:?}", key);
    //     println!("SEEK ACCOUNT :{:?}", decoded_account);

    //     let seek_response = scalerize_client.seek(0, cursor_id_2.to_vec(), key4.as_slice()).
    //     expect("Failed to get seek_exact for accounts");
    //     let key = seek_response.0;
    //     let encoded_account = seek_response.1;
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("SEEK ACCOUNT KEY: {:?}", key);
    //     println!("SEEK ACCOUNT :{:?}", decoded_account);

    //     let next_response = scalerize_client.next(0, cursor_id_3.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = next_response.0;
    //     let encoded_account = next_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("NEXT ACCOUNT KEY: {:?}", key);
    //     println!("NEXT ACCOUNT :{:?}", decoded_account);

    //     let next_response = scalerize_client.next(0, cursor_id_3.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = next_response.0;
    //     let encoded_account = next_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("NEXT ACCOUNT KEY: {:?}", key);
    //     println!("NEXT ACCOUNT :{:?}", decoded_account);

    //     let next_response = scalerize_client.next(0, cursor_id_3.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = next_response.0;
    //     let encoded_account = next_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("NEXT ACCOUNT KEY: {:?}", key);
    //     println!("NEXT ACCOUNT :{:?}", decoded_account);

    //     let prev_response = scalerize_client.prev(0, cursor_id_3.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = prev_response.0;
    //     let encoded_account = prev_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("PREV ACCOUNT KEY: {:?}", key);
    //     println!("PREV ACCOUNT :{:?}", decoded_account);

    //     let prev_response = scalerize_client.prev(0, cursor_id_3.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = prev_response.0;
    //     let encoded_account = prev_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("PREV ACCOUNT KEY: {:?}", key);
    //     println!("PREV ACCOUNT :{:?}", decoded_account);

    //     // let prev_response = scalerize_client.prev(0, cursor_id_3.to_vec(), 32).
    //     // expect("Failed to get first entry for accounts");

    //     // let key = prev_response.0;
    //     // let encoded_account = prev_response.1;

    //     // println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     // let rlp = Rlp::new(&encoded_account);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     // println!("PREV ACCOUNT KEY: {:?}", key);
    //     // println!("PREV ACCOUNT :{:?}", decoded_account);

    //     // let prev_response = scalerize_client.prev(0, cursor_id_3.to_vec(), 32).
    //     // expect("Failed to get first entry for accounts");

    //     // let key = prev_response.0;
    //     // let encoded_account = prev_response.1;

    //     // println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     // let rlp = Rlp::new(&encoded_account);
    //     // let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     // println!("PREV ACCOUNT KEY: {:?}", key);
    //     // println!("PREV ACCOUNT :{:?}", decoded_account);

    //     let prev_response = scalerize_client.prev(0, cursor_id_4.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = prev_response.0;
    //     let encoded_account = prev_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("PREV ACCOUNT KEY: {:?}", key);
    //     println!("PREV ACCOUNT :{:?}", decoded_account);

    //     let prev_response = scalerize_client.prev(0, cursor_id_4.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = prev_response.0;
    //     let encoded_account = prev_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("PREV ACCOUNT KEY: {:?}", key);
    //     println!("PREV ACCOUNT :{:?}", decoded_account);

    //     let last_response = scalerize_client.last(0, cursor_id_4.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = last_response.0;
    //     let encoded_account = last_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("LAST ACCOUNT KEY: {:?}", key);
    //     println!("LAST ACCOUNT :{:?}", decoded_account);

    //     let current_response = scalerize_client.current(0, cursor_id_4.to_vec(), 32).
    //     expect("Failed to get first entry for accounts");

    //     let key = current_response.0;
    //     let encoded_account = current_response.1;

    //     println!("BYTES TO BE DECODED:{:?}", &encoded_account);
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");

    //     println!("CURRENT ACCOUNT KEY: {:?}", key);
    //     println!("CURRENT ACCOUNT :{:?}", decoded_account);

    //     let first_response = scalerize_client.first(1, cursor_id_6.to_vec(), 32).
    //     expect("Failed to get first entry for Storages");
    //     let key = first_response.0;
    //     let value = first_response.1;
    //     let storage_entry = StorageEntry{
    //         key: B256::from_slice(&value[..32]),
    //         value:U256::from_be_slice(&value[32..]),
    //     };

    //     println!("FIRST KEY: {:?}", key);
    //     println!("FIRST Storage Entry:{:?}", storage_entry);

    //     let seek_exact_response = scalerize_client.seek_exact(1, cursor_id_7.to_vec(),
    // key2.as_slice()).     expect("Failed to get seek_exact for storages");

    //     if !seek_exact_response.is_empty() {
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_exact_response[..32]),
    //             value:U256::from_be_slice(&seek_exact_response[32..]),
    //         };

    //         println!("SEEK EXACT KEY: {:?}", key2);
    //         println!("SEEK EXACT Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_exact_response = scalerize_client.seek_exact(1, cursor_id_7.to_vec(),
    // key1.as_slice()).     expect("Failed to get seek_exact for storages");

    //     if !seek_exact_response.is_empty() {
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_exact_response[..32]),
    //             value:U256::from_be_slice(&seek_exact_response[32..]),
    //         };

    //         println!("SEEK EXACT KEY: {:?}", key1);
    //         println!("SEEK EXACT Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_exact_response = scalerize_client.seek_exact(1, cursor_id_7.to_vec(),
    // key3.as_slice()).     expect("Failed to get seek_exact for storages");

    //     if !seek_exact_response.is_empty() {
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_exact_response[..32]),
    //             value:U256::from_be_slice(&seek_exact_response[32..]),
    //         };

    //         println!("SEEK EXACT KEY: {:?}", key3);
    //         println!("SEEK EXACT Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK_EXACT response is empty");
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_7.to_vec(), key2.as_slice()).
    //     expect("Failed to get seek_exact for storages");
    //     println!("CURRENT ON NON SET CURSOR: {:?}", scalerize_client.current(1,
    // cursor_id_7.to_vec(), 32));

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_7.to_vec(), key1.as_slice()).
    //     expect("Failed to get seek_exact for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next(1, cursor_id_7.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     loop {
    //         match scalerize_client.prev(1, cursor_id_7.to_vec(), 32) {
    //             Ok(prev_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&prev_response.1[..32]),
    //                     value: U256::from_be_slice(&prev_response.1[32..]),
    //                 };

    //                 println!("PREV KEY: {:?}", prev_response.0);
    //                 println!("PREV Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get prev for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_7.to_vec(), key1.as_slice()).
    //     expect("Failed to get seek_exact for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     println!("CURRENT ON NON SET CURSOR: {:?}", scalerize_client.current(1,
    // cursor_id_8.to_vec(), 32));

    //     let b256_var_from_bytes12 = B256::from([
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes22 = B256::from([
    //         1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes32 = B256::from([
    //         2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes42 = B256::from([
    //         3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes52 = B256::from([
    //         4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes62 = B256::from([
    //         1u8, 1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes72 = B256::from([
    //         1u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes82 = B256::from([
    //         1u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes92 = B256::from([
    //         5u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let b256_var_from_bytes102 = B256::from([
    //         2u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
    //         0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 58u8 // Example bytes
    //     ]);

    //     let value12 = Account {
    //         nonce: 5,
    //         balance: U256::from(1000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let value22 = Account {
    //         nonce: 6,
    //         balance: U256::from(2000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let value32 = Account {
    //         nonce: 7,
    //         balance: U256::from(3000),
    //         bytecode_hash: Some(U256::from(12345).into()),
    //     };

    //     let entry12 = StorageEntry { key: b256_var_from_bytes12, value: value1 };
    //     let entry22 = StorageEntry { key: b256_var_from_bytes22, value: value2 };
    //     let entry32 = StorageEntry { key: b256_var_from_bytes32, value: value3 };

    //     let encoded_account1 = rlp::encode(&value12);
    //     let encoded_account2 = rlp::encode(&value22);
    //     let encoded_account3 = rlp::encode(&value32);
    //     let upsert_response = scalerize_client.upsert(0, cursor_id_8.to_vec(),
    // b256_var_from_bytes82.as_slice(), None, &encoded_account1);

    //     println!("UPSERT RESPONSE: {:?}", upsert_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(0, cursor_id_7.to_vec(),
    // b256_var_from_bytes82.as_slice()).     expect("Failed to get seek for accounts");

    //     let key = seek_response.0;
    //     let encoded_account = seek_response.1;
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("SEEK ACCOUNT KEY: {:?}", key);
    //     println!("SEEK ACCOUNT :{:?}", decoded_account);

    //     let seek_response = scalerize_client.seek_exact(0, cursor_id_7.to_vec(),
    // b256_var_from_bytes82.as_slice()).     expect("Failed to get seek for accounts");

    //     // let key = seek_response.0;
    //     let encoded_account = seek_response;
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("SEEK EXACT ACCOUNT KEY: {:?}", key);
    //     println!("SEEK EXACT ACCOUNT :{:?}", decoded_account);

    //     let upsert_response = scalerize_client.upsert(1, cursor_id_8.to_vec(),
    // b256_var_from_bytes12.as_slice(), Some(entry12.key.as_slice()),
    // &entry12.value.to_be_bytes::<32>());

    //     println!("UPSERT RESPONSE: {:?}", upsert_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_7.to_vec(),
    // b256_var_from_bytes12.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next(0, cursor_id_9.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let key = next_response.0;
    //                 let encoded_account = next_response.1;
    //                 let rlp = Rlp::new(&encoded_account);
    //                 let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //                 println!("NEXT ACCOUNT KEY: {:?}", key);
    //                 println!("NEXT ACCOUNT :{:?}", decoded_account);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     // this fails since account with key already present in the table
    //     println!("INSERT RESPONSE: {:?}", scalerize_client.insert(0, cursor_id_8.to_vec(),
    // b256_var_from_bytes82.as_slice(), None, &encoded_account2));

    //     let insert_response = scalerize_client.insert(0, cursor_id_8.to_vec(),
    // b256_var_from_bytes72.as_slice(), None, &encoded_account2).     expect("Failed to insert
    // for accounts");

    //     println!("INSERT RESPONSE: {:?}", insert_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(0, cursor_id_7.to_vec(),
    // b256_var_from_bytes72.as_slice()).     expect("Failed to get seek for accounts");

    //     let key = seek_response.0;
    //     let encoded_account = seek_response.1;
    //     let rlp = Rlp::new(&encoded_account);
    //     let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //     println!("SEEK ACCOUNT KEY: {:?}", key);
    //     println!("SEEK ACCOUNT :{:?}", decoded_account);

    //     // this fails since account with key already present in the table
    //     println!("INSERT RESPONSE IF KEY AND SUBKEY IS SAME: {:?}", scalerize_client.insert(1,
    // cursor_id_8.to_vec(), b256_var_from_bytes12.as_slice(), Some(entry12.key.as_slice()),
    // &entry12.value.to_be_bytes::<32>()));     println!("INSERT RESPONSE IF ONLY KEY IS SAME:
    // {:?}", scalerize_client.insert(1, cursor_id_8.to_vec(), b256_var_from_bytes12.as_slice(),
    // Some(entry32.key.as_slice()), &entry32.value.to_be_bytes::<32>()));

    //     let insert_response = scalerize_client.insert(1, cursor_id_8.to_vec(),
    // b256_var_from_bytes72.as_slice(),Some(entry32.key.as_slice()),
    // &entry32.value.to_be_bytes::<32>()).     expect("Failed to insert for accounts");

    //     println!("INSERT RESPONSE: {:?}", insert_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_7.to_vec(),
    // b256_var_from_bytes72.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next(0, cursor_id_10.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let key = next_response.0;
    //                 let encoded_account = next_response.1;
    //                 let rlp = Rlp::new(&encoded_account);
    //                 let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //                 println!("NEXT ACCOUNT KEY: {:?}", key);
    //                 println!("NEXT ACCOUNT :{:?}", decoded_account);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("APPEND RESPONSE WHEN KEY IS LESS THAN GREATEST KEY: {:?}",
    // scalerize_client.append(0, cursor_id_8.to_vec(), b256_var_from_bytes62.as_slice(), None,
    // &encoded_account1));     println!("APPEND RESPONSE WHEN KEY IS EQUAL TO THE GREATEST KEY:
    // {:?}", scalerize_client.append(0, cursor_id_8.to_vec(), b256_var_from_bytes52.as_slice(),
    // None, &encoded_account1));

    //     println!("UPSERT RESPONSE: {:?}", upsert_response);

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     loop {
    //         match scalerize_client.next(0, cursor_id_10.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let key = next_response.0;
    //                 let encoded_account = next_response.1;
    //                 let rlp = Rlp::new(&encoded_account);
    //                 let decoded_account = Account::decode(&rlp).expect("Failed to decode");
    //                 println!("NEXT ACCOUNT KEY: {:?}", key);
    //                 println!("NEXT ACCOUNT :{:?}", decoded_account);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for accounts: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("all for hashed storages");

    //     loop {
    //         match scalerize_client.next(1, cursor_id_11.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("APPEND RESPONSE WHEN KEY IS LESS THAN GREATEST KEY: {:?}",
    // scalerize_client.append(1, cursor_id_8.to_vec(), b256_var_from_bytes22.as_slice(),
    // Some(entry32.key.as_slice()), &entry32.value.to_be_bytes::<32>()));     println!("APPEND
    // RESPONSE WHEN KEY IS EQUAL TO THE GREATEST KEY AND SUBKEY LESS: {:?}",
    // scalerize_client.append(1, cursor_id_8.to_vec(), b256_var_from_bytes3.as_slice(),
    // Some(entry32.key.as_slice()), &entry32.value.to_be_bytes::<32>()));     println!("APPEND
    // RESPONSE WHEN KEY IS EQUAL TO THE GREATEST KEY AND SUBKEY GREATEST: {:?}",
    // scalerize_client.append(1, cursor_id_8.to_vec(), b256_var_from_bytes3.as_slice(),
    // Some(entry3.key.as_slice()), &entry3.value.to_be_bytes::<32>()));     println!("APPEND
    // RESPONSE WHEN KEY IS EQUAL TO THE GREATEST KEY AND SUBKEY GREATEST: {:?}",
    // scalerize_client.append(1, cursor_id_8.to_vec(), b256_var_from_bytes4.as_slice(),
    // Some(entry3.key.as_slice()), &entry3.value.to_be_bytes::<32>()));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     loop {
    //         match scalerize_client.next(1, cursor_id_12.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     loop {
    //         match scalerize_client.next_dup(1, cursor_id_13.to_vec(), 32) {
    //             Ok(Some((key, value))) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT DUP KEY: {:?}", key);
    //                 println!("NEXT DUP Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT DUP response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_13.to_vec(), 32));

    //     let seek_response = scalerize_client.seek(1, cursor_id_13.to_vec(),
    // b256_var_from_bytes12.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next_dup(1, cursor_id_13.to_vec(), 32) {
    //             Ok(Some((key, value))) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT DUP KEY: {:?}", key);
    //                 println!("NEXT DUP Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT DUP response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_13.to_vec(), 32));

    //     let seek_response = scalerize_client.seek(1, cursor_id_13.to_vec(),
    // b256_var_from_bytes72.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next_dup(1, cursor_id_13.to_vec(), 32) {
    //             Ok(Some((key, value))) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT DUP KEY: {:?}", key);
    //                 println!("NEXT DUP Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT DUP response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_13.to_vec(), 32));

    //     let seek_response = scalerize_client.seek(1, cursor_id_13.to_vec(),
    // b256_var_from_bytes3.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry{
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     loop {
    //         match scalerize_client.next_dup(1, cursor_id_13.to_vec(), 32) {
    //             Ok(Some((key, value))) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT DUP KEY: {:?}", key);
    //                 println!("NEXT DUP Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT DUP response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_13.to_vec(), 32));

    //     loop {
    //         match scalerize_client.next_no_dup(1, cursor_id_14.to_vec(), 32) {
    //             Ok(Some((key, value))) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT NO DUP KEY: {:?}", key);
    //                 println!("NEXT NO DUP Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT NO DUP response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_14.to_vec(), 32));

    //     loop {
    //         match scalerize_client.next_dup_val(1, cursor_id_15.to_vec()) {
    //             Ok(Some(value)) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&value[..32]),
    //                     value: U256::from_be_slice(&value[32..]),
    //                 };

    //                 println!("NEXT DUP VAL Storage Entry: {:?}", storage_entry);
    //             }
    //             Ok(None) => {
    //                 println!("NEXT DUP VAL response is empty");
    //                 break;
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next dup for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_15.to_vec(), 32));

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_25.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(),
    // b256_var_from_bytes6.as_slice(), b256_var_from_bytes12.as_slice()) {
    //         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(), key1.as_slice(),
    // b256_var_from_bytes12.as_slice()) {         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(), key1.as_slice(),
    // b256_var_from_bytes92.as_slice()) {         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(), key3.as_slice(),
    // b256_var_from_bytes92.as_slice()) {         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(), key3.as_slice(),
    // b256_var_from_bytes102.as_slice()) {         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));

    //     match scalerize_client.seek_by_key_subkey(1, cursor_id_16.to_vec(),
    // b256_var_from_bytes10.as_slice(), b256_var_from_bytes102.as_slice()) {
    //         Ok(Some(value)) => {
    //             let storage_entry = StorageEntry {
    //                 key: B256::from_slice(&value[..32]),
    //                 value: U256::from_be_slice(&value[32..]),
    //             };

    //             println!("SEEK BY KEY SUBKEY Storage Entry: {:?}", storage_entry);
    //         }
    //         Ok(None) => {
    //             println!("SEEK BY KEY SUBKEY response is empty");
    //         }
    //         Err(e) => {
    //             println!("Failed to get next dup for storages: {:?}", e);
    //         }
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_16.to_vec(), 32));
    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_17.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     println!("DELETE CURRENT DUPLICATES: {:?}", scalerize_client.delete_current_duplicates(1,
    // cursor_id_18.to_vec()));     println!("CURRENT: {:?}", scalerize_client.current(1,
    // cursor_id_18.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     let seek_response = scalerize_client.seek(1, cursor_id_18.to_vec(),
    // b256_var_from_bytes1.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry {
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_18.to_vec(), 32));

    //     println!("DELETE CURRENT DUPLICATES: {:?}", scalerize_client.delete_current_duplicates(1,
    // cursor_id_18.to_vec()));     println!("CURRENT: {:?}", scalerize_client.current(1,
    // cursor_id_18.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_19.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     let seek_response = scalerize_client.seek(1, cursor_id_19.to_vec(),
    // b256_var_from_bytes4.as_slice()).     expect("Failed to get seek for storages");

    //     if !seek_response.0.is_empty() && !seek_response.1.is_empty(){
    //         let storage_entry = StorageEntry {
    //             key: B256::from_slice(&seek_response.1[..32]),
    //             value:U256::from_be_slice(&seek_response.1[32..]),
    //         };

    //         println!("SEEK KEY: {:?}", seek_response.0);
    //         println!("SEEK Storage Entry:{:?}", storage_entry);
    //     } else {
    //         println!("SEEK response is empty");
    //     }

    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_19.to_vec(), 32));

    //     println!("DELETE CURRENT DUPLICATES: {:?}", scalerize_client.delete_current_duplicates(1,
    // cursor_id_19.to_vec()));     println!("CURRENT: {:?}", scalerize_client.current(1,
    // cursor_id_19.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_20.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     println!("APPEND DUP: {:?}", scalerize_client.append_dup(1, cursor_id_21.to_vec(),
    // b256_var_from_bytes12.as_slice(), b256_var_from_bytes1.as_slice(), &1u8.to_be_bytes()));
    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_21.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("APPEND DUP: {:?}", scalerize_client.append_dup(1, cursor_id_21.to_vec(),
    // b256_var_from_bytes3.as_slice(), b256_var_from_bytes4.as_slice(), &13u8.to_be_bytes()));
    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_21.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_22.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     println!("APPEND DUP: {:?}", scalerize_client.append_dup(1, cursor_id_21.to_vec(),
    // b256_var_from_bytes3.as_slice(), b256_var_from_bytes5.as_slice(), &16u8.to_be_bytes()));
    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_21.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_23.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");

    //     println!("APPEND DUP: {:?}", scalerize_client.append_dup(1, cursor_id_21.to_vec(),
    // b256_var_from_bytes6.as_slice(), b256_var_from_bytes5.as_slice(), &20u8.to_be_bytes()));
    //     println!("CURRENT: {:?}", scalerize_client.current(1, cursor_id_21.to_vec(), 32));

    //     match scalerize_client.write() {
    //         Ok(_) => {
    //             println!("WRITTEN TO DB");
    //         }
    //         Err(e) => {
    //             println!("Write request failed with error: {:?}", e);
    //         }
    //     }

    //     println!("-----------");
    //     loop {
    //         match scalerize_client.next(1, cursor_id_24.to_vec(), 32) {
    //             Ok(next_response) => {
    //                 let storage_entry = StorageEntry {
    //                     key: B256::from_slice(&next_response.1[..32]),
    //                     value: U256::from_be_slice(&next_response.1[32..]),
    //                 };

    //                 println!("NEXT KEY: {:?}", next_response.0);
    //                 println!("NEXT Storage Entry: {:?}", storage_entry);
    //             }
    //             Err(e) => {
    //                 println!("Failed to get next for storages: {:?}", e);
    //                 break;
    //             }
    //         }
    //     }
    //     println!("-----------");
    //     Ok(())
    // }

    #[test]
    fn db_cursor_insert_dup() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let key = Address::random();
        let subkey1 = B256::random();
        let subkey2 = B256::random();

        let entry1 = StorageEntry { key: subkey1, value: U256::ZERO };
        assert!(dup_cursor.insert(key, entry1).is_ok());

        // Can't insert
        let entry2 = StorageEntry { key: subkey2, value: U256::ZERO };
        assert!(dup_cursor.insert(key, entry2).is_err());
    }

    #[test]
    fn db_cursor_delete_current_non_existent() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let key1 = Address::with_last_byte(1);
        let key2 = Address::with_last_byte(2);
        let key3 = Address::with_last_byte(3);
        let key4 = Address::with_last_byte(4);
        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();

        assert!(cursor.insert(key1, Account::default()).is_ok());
        assert!(cursor.insert(key3, Account::default()).is_ok());
        assert!(cursor.insert(key4, Account::default()).is_ok());
        // assert!(cursor.insert(key4, Account::default()).is_ok());

        let b256_var_from_bytes1 = B256::from([
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes2 = B256::from([
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes3 = B256::from([
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes4 = B256::from([
            3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes5 = B256::from([
            4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes6 = B256::from([
            1u8, 1u8, 1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes7 = B256::from([
            1u8, 1u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes8 = B256::from([
            1u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes9 = B256::from([
            2u8, 2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes10 = B256::from([
            4u8, 4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        // println!("for hashed storage");
        let subkey = B256::random();
        // let subkey: H256 = H256::zero();
        // println!("subkey {:?}", subkey);

        let value1 = U256::from(1);
        let value2 = U256::from(2);
        let value3 = U256::from(3);
        let value4 = U256::from(4);
        let entry1 = StorageEntry { key: b256_var_from_bytes2, value: value1 };
        let entry2 = StorageEntry { key: b256_var_from_bytes3, value: value2 };
        let entry3 = StorageEntry { key: b256_var_from_bytes4, value: value3 };
        let entry4 = StorageEntry { key: b256_var_from_bytes5, value: value4 };

        let entry5 = StorageEntry { key: b256_var_from_bytes6, value: value1 };
        let entry6 = StorageEntry { key: b256_var_from_bytes7, value: value2 };
        let entry7 = StorageEntry { key: b256_var_from_bytes8, value: value3 };
        let entry8 = StorageEntry { key: b256_var_from_bytes9, value: value4 };
        let entry9 = StorageEntry { key: b256_var_from_bytes2, value: value4 };
        let entry10 = StorageEntry { key: b256_var_from_bytes2, value: value2 };
        let entry11 = StorageEntry { key: b256_var_from_bytes9, value: value2 };
        let entry12 = StorageEntry { key: b256_var_from_bytes9, value: value4 };

        let mut cursor = tx.cursor_write::<HashedStorages>().unwrap();
        println!("CURRENT 1: {:?}", cursor.current());
        assert!(cursor.insert(b256_var_from_bytes1, entry1).is_ok());
        // assert!(tx.put(b256_var_from_bytes1, entry2).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes1, entry3).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes1, entry4).is_ok());
        println!("CURRENT 2: {:?}", cursor.current());
        assert!(cursor.insert(b256_var_from_bytes2, entry5).is_ok());
        // assert!(cursor.insert(b256_var_from_bytes2, entry5).is_ok());
        // assert!(cursor.insert(b256_var_from_bytes2, entry9).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes3, entry6).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes3, entry7).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes3, entry8).is_ok());
        println!("CURRENT 3: {:?}", cursor.current());

        let mut cursorrw = tx.cursor_write::<HashedStorages>().unwrap();

        println!("SEEK EXACT: {:?}", cursor.seek_exact(b256_var_from_bytes1));
        // assert!(cursorrw.upsert(b256_var_from_bytes1, entry9).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes1, entry9).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes1, entry1).is_ok());
        assert!(tx.put::<HashedStorages>(b256_var_from_bytes1, entry10).is_ok());
        println!("SEEK EXACT: {:?}", cursor.seek_exact(b256_var_from_bytes1));

        let mut cursor3 = tx.cursor_write::<HashedStorages>().unwrap();
        while let Some((key, value)) = cursor3.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }
        println!("NEXT AFTER NEXT IS CALLED FOR THE LAST ENTRY: {:?}", cursor3.next());
        println!("CURRENT AFTER NEXT IS CALLED FOR THE LAST ENTRY: {:?}", cursor3.current());
        println!("CURRENT 5: {:?}", cursorrw.current());
        assert!(cursorrw.upsert(b256_var_from_bytes4, entry5).is_ok());
        println!("CURRENT 6: {:?}", cursorrw.current());

        println!("HERE {:?}", cursorrw.current());
        while let Some((key, value)) = cursorrw.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }
        println!("CURSOR CURRENT: {:?}", cursorrw.current());

        println!("CURRENT BEFORE DELETE CURRENT THE LAST: {:?}", cursorrw.current());
        assert_eq!(cursorrw.delete_current(), Ok(()));
        println!("CURRENT AFTER DELETE CURRENT THE LAST: {:?}", cursorrw.current());

        while let Some((key, value)) = cursorrw.prev().unwrap() {
            println!("PREV Key: {}, Value: {:?}", key, value);
        }

        println!("SEEK EXACT: {:?}", cursorrw.seek_exact(b256_var_from_bytes3));
        println!("CURRENT BEFORE DELETE CURRENT: {:?}", cursorrw.current());
        assert_eq!(cursorrw.delete_current(), Ok(()));
        println!("CURRENT AFTER DELETE CURRENT: {:?}", cursorrw.current());

        let mut cursor5 = tx.cursor_write::<HashedStorages>().unwrap();
        while let Some((key, value)) = cursor5.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }

        let mut cursor6 = tx.cursor_write::<HashedStorages>().unwrap();
        println!("KEY: {:?}", b256_var_from_bytes2);
        println!("ENTRY: {:?}", entry4);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes2, entry4));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes3);
        println!("ENTRY: {:?}", entry1);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes3, entry1));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes3);
        println!("ENTRY: {:?}", entry10);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes3, entry10));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes10);
        println!("ENTRY: {:?}", entry10);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes10, entry10));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes10);
        println!("ENTRY: {:?}", entry10);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes10, entry10));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes10);
        println!("ENTRY: {:?}", entry12);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes10, entry12));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        println!("KEY: {:?}", b256_var_from_bytes10);
        println!("ENTRY: {:?}", entry11);
        println!("CURSOR APPEND: {:?}", cursor6.append(b256_var_from_bytes10, entry11));
        println!("CURSOR CURRENT: {:?}", cursor6.current());

        let mut cursor6 = tx.cursor_write::<HashedStorages>().unwrap();
        while let Some((key, value)) = cursor6.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }

        let mut cursor7 = tx.cursor_write::<HashedAccounts>().unwrap();

        let key1 = B256::from([
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let key2 = B256::from([
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let key3 = B256::from([
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let key4 = B256::from([
            2u8, 2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let value1 = Account { nonce: 1, bytecode_hash: Some(B256::random()), balance: U256::MAX };

        let value2 = Account { nonce: 2, bytecode_hash: Some(B256::random()), balance: U256::MAX };

        let value3 = Account { nonce: 3, bytecode_hash: Some(B256::random()), balance: U256::MAX };

        let value4 = Account { nonce: 4, bytecode_hash: Some(B256::random()), balance: U256::MAX };
        println!("CURSOR APPEND: {:?}", cursor7.append(key1, value1));
        println!("CURSOR CURRENT: {:?}", cursor7.current());

        println!("CURSOR APPEND: {:?}", cursor7.append(key2, value2));
        println!("CURSOR CURRENT: {:?}", cursor7.current());

        println!("CURSOR APPEND: {:?}", cursor7.append(key4, value3));
        println!("CURSOR CURRENT: {:?}", cursor7.current());

        println!("CURSOR APPEND: {:?}", cursor7.append(key4, value4));
        println!("CURSOR CURRENT: {:?}", cursor7.current());

        let mut cursor6 = tx.cursor_write::<HashedAccounts>().unwrap();
        while let Some((key, value)) = cursor6.next().unwrap() {
            println!("NEXT Key: {}, Value: {:?}", key, value);
        }

        println!("CURSOR APPEND: {:?}", cursor7.append(key4, value1));
        println!("CURSOR CURRENT: {:?}", cursor7.current());

        // println!("CURSOR CURRENT: {:?}", cursorrw.current());
        // let current = cursorrw.current().
        // expect("current failed");
        // println!("CURRENT: {:?}", current);

        // assert_eq!(cursorrw.delete_current(), Ok(()));
        // println!("{:?}", cursorrw.current());

        // cursor.seek_exact(key4).unwrap();
        // println!("{:?}", cursor.current());
        // let current = cursor.current().
        // expect("current failed");
        // println!("CURRENT BEFORE: {:?}", current);
        // assert_eq!(cursor.delete_current(), Ok(()));
        // let current = cursor.current().
        // expect("currrent failed");
        // println!("CURRENT AFTER: {:?}", current);
        // assert_eq!(cursor.delete_current(), Ok(()));
        // let current = cursor.current().
        // expect("current failed");
        // println!("CURRENT AFTER: {:?}", current);
        // assert_eq!(cursor.delete_current(), Ok(()));
        // let current = cursor.current().
        // expect("current failed");
        // println!("CURRENT AFTER: {:?}", current);

        // assert_eq!(cursor.seek_exact(key2), Ok(None));
        // println!("{:?}", cursor.current());

        // Seek & delete key2
        // cursor.seek_exact(key2).unwrap();
        // println!("{:?}", cursor.current());
        // let current = cursor.current().
        // expect("currrent failed");
        // println!("CURRENT BEFORE: {:?}", current);
        // assert_eq!(cursor.delete_current(), Ok(()));
        // let current = cursor.current().
        // expect("currrent failed");
        // println!("CURRENT AFTER: {:?}", current);
        // assert_eq!(cursor.seek_exact(key2), Ok(None));
        // println!("{:?}", cursor.current());

        // Seek & delete key2 again
        // assert_eq!(cursor.seek_exact(key2), Ok(None));
        // assert_eq!(cursor.delete_current(), Ok(()));
        // // Assert that key1 is still there
        // assert_eq!(cursor.seek_exact(key1), Ok(Some((key1, Account::default()))));
        // // Assert that key3 was deleted
        // assert_eq!(cursor.seek_exact(key3), Ok(None));
    }

    #[test]
    fn db_cursor_insert_wherever_cursor_is() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let pavalue = Account {
            nonce: 18446744073709551615,
            bytecode_hash: Some(B256::random()),
            balance: U256::MAX,
        };
        let pakey = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // println!("for plain account state");
        // vec![0, 1, 3, 5, 7, 9]
        // .into_iter()
        // .try_for_each(|key| tx.put::<PlainAccountState>(pakey, pavalue))
        // .expect(ERROR_PUT);

        // let b256_var_from_bytes = B256::from([
        //     0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        //     0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        //     0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
        //     0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8, 57u8 // Example bytes
        // ]);

        let b256_var_from_bytes1 = B256::from([
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes2 = B256::from([
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes3 = B256::from([
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes4 = B256::from([
            3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        let b256_var_from_bytes5 = B256::from([
            4u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 3u8,
            57u8, // Example bytes
        ]);

        println!("for hashed storage");
        // let subkey = B256::random();
        let subkey: H256 = H256::zero();
        println!("subkey {:?}", subkey);

        let value1 = U256::from(1);
        let value2 = U256::from(2);
        let value3 = U256::from(3);
        let value4 = U256::from(4);
        let entry1 = StorageEntry { key: b256_var_from_bytes2, value: value1 };
        let entry2 = StorageEntry { key: b256_var_from_bytes3, value: value2 };
        let entry3 = StorageEntry { key: b256_var_from_bytes4, value: value3 };
        let entry4 = StorageEntry { key: b256_var_from_bytes5, value: value4 };

        // tx.put::<HashedStorages>(b256_var_from_bytes1, entry1)
        // .expect(ERROR_PUT);

        tx.put::<HashedStorages>(b256_var_from_bytes1, entry1).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry2).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry3).expect(ERROR_PUT);
        tx.put::<HashedStorages>(b256_var_from_bytes1, entry4).expect(ERROR_PUT);

        let result = tx.get::<HashedStorages>(b256_var_from_bytes1).expect(ERROR_GET);

        println!("STORAGE ENTRY1: {:?}", result);

        let result =
            tx.delete::<HashedStorages>(b256_var_from_bytes1, Some(entry1)).expect(ERROR_GET);

        let result = tx.get::<HashedStorages>(b256_var_from_bytes1).expect(ERROR_GET);

        println!("STORAGE ENTRY2: {:?}", result);
        // vec![0, 1, 3, 5, 7, 9]
        // .into_iter()
        // .try_for_each(|key| tx.put::<HeaderTerminalDifficulties>(key, B256::ZERO))
        // .expect(ERROR_PUT);
        println!("FOR HASHED ACCOUNTS");

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account =
            Account { nonce: 9, balance: max_u256, bytecode_hash: Some(U256::from(12345).into()) };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account {
            nonce: 9,
            balance: U256::from(1),
            bytecode_hash: Some(U256::from(12345).into()),
        };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account {
            nonce: 0,
            balance: U256::from(12345),
            bytecode_hash: Some(U256::from(12345).into()),
        };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 0, balance: U256::from(256), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 9, balance: U256::from(1), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 9, balance: max_u256, bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        println!();

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account {
            nonce: 0,
            balance: U256::from(0),
            bytecode_hash: Some(U256::from(12345).into()),
        };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account {
            nonce: 1,
            balance: U256::from(0),
            bytecode_hash: Some(U256::from(12345).into()),
        };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account {
            nonce: 0,
            balance: U256::from(1),
            bytecode_hash: Some(U256::from(12345).into()),
        };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 1, balance: U256::from(0), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 3, balance: U256::from(0), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 9, balance: U256::from(0), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 0, balance: U256::from(1), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account =
            Account { nonce: 18446744073709551615, balance: U256::from(0), bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        let max_u256 = U256::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        let account: Account = Account { nonce: 0, balance: max_u256, bytecode_hash: None };
        tx.put::<HashedAccounts>(B256::random(), account).expect(ERROR_PUT);

        tx.commit().expect(ERROR_COMMIT);
        // PUT
        // vec![0, 1, 3, 5, 7, 9]
        //     .into_iter()
        //     .try_for_each(|key| tx.put::<HashedAccounts>(B256::random(), account))
        //     .expect(ERROR_PUT);
        // tx.commit().expect(ERROR_COMMIT);

        // vec![
        //     B256::from([0u8; 32]),
        //     B256::from([1u8; 32]),
        //     B256::from([3u8; 32]),
        //     B256::from([5u8; 32]),
        //     B256::from([7u8; 32]),
        //     B256::from([9u8; 32]),
        // ]
        // .into_iter()
        // .try_for_each(|key: alloy_primitives::FixedBytes<32>| tx.put::<HashedAccounts>(key,
        // account)) .expect(ERROR_PUT);
        // tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursorrw = tx.cursor_write::<HashedStorages>().unwrap();
        let mut entries = Vec::new();
        while let Some((key, value)) = cursorrw.next().unwrap() {
            entries.push((key, value));
        }

        for (key, value) in entries {
            println!("Key: {:?}, Value: {:?}", key, value);
        }

        let mut cursorrw = tx.cursor_write::<HashedStorages>().unwrap();
        let mut entries = Vec::new();
        while let Some((key, value)) = cursorrw.next().unwrap() {
            entries.push((key, value));
        }

        for (key, value) in entries {
            println!("Key: {:?}, Value: {:?}", key, value);
        }

        let bool =
            tx.delete::<HashedStorages>(b256_var_from_bytes1, Some(entry3)).expect(ERROR_DEL);
        println!("{}", bool);

        println!("AFTER DELETE");
        let mut cursorrw = tx.cursor_write::<HashedStorages>().unwrap();
        let mut entries = Vec::new();
        while let Some((key, value)) = cursorrw.next().unwrap() {
            entries.push((key, value));
        }

        for (key, value) in entries {
            println!("Key: {:?}, Value: {:?}", key, value);
        }
        // let mut cursor1 = tx.cursor_write::<HashedAccounts>().unwrap();
        // INSERT (cursor starts at last)
        // cursor.last().unwrap();
        // assert_eq!(cursor.current(), Ok(Some((9, B256::ZERO))));

        // for pos in (2..=8).step_by(2) {
        //     assert_eq!(cursor.insert(pos, B256::ZERO), Ok(()));
        //     assert_eq!(cursor.current(), Ok(Some((pos, B256::ZERO))));
        // }
        // tx.commit().expect(ERROR_COMMIT);

        // // Confirm the result
        // let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursorro = tx.cursor_read::<HashedStorages>().unwrap();
        // let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        // assert_eq!(res, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        // tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        // assert_eq!(cursor.append(key_to_append, B256::ZERO), Ok(()));
        println!("RESPONSE: {:?}", cursor.append(key_to_append,  B256::ZERO));

        tx.commit().expect(ERROR_COMMIT);
        // let tx = db.tx_mut().expect(ERROR_INIT_TX);
        // let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();

        let key_to_append = 4;

        println!("RESPONSE: {:?}", cursor.append(key_to_append,  B256::ZERO));
        // Confirm the result
        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let res = cursor.walk(None).unwrap().map(|res| res.unwrap().0).collect::<Vec<_>>();
        assert_eq!(res, vec![0, 1, 2, 3, 4, 5]);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_append_failure() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(
            cursor.append(key_to_append, B256::ZERO),
            Err(DatabaseWriteError {
                info: Error::KeyMismatch.into(),
                operation: DatabaseWriteOperation::CursorAppend,
                table_name: CanonicalHeaders::NAME,
                key: key_to_append.encode().into(),
            }
            .into())
        );
        assert_eq!(cursor.current(), Ok(Some((5, B256::ZERO)))); // the end of table
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
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let tx = db.tx_mut().expect(ERROR_INIT_TX);

        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();
        let key = Address::random();
        let key1 = U160::from_be_slice(&[
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 3u8, 57u8, // Example bytes
        ]);
        let key2 = U160::from_be_slice(&[
            1u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 3u8, 57u8, // Example bytes
        ]);
        let key3 = U160::from_be_slice(&[
            2u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 3u8, 57u8, // Example bytes
        ]);
        let key4 = U160::from_be_slice(&[
            3u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8,
            0u8, 3u8, 57u8, // Example bytes
        ]);
        println!("CURRENT {:?}", cursor.current());

        let account = Account::default();

        cursor.upsert(key1.into(), account).expect(ERROR_UPSERT);
        println!("CURRENT {:?}", cursor.current());

        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));
        println!("CURRENT {:?}", cursor.current());

        let account = Account { nonce: 1, ..Default::default() };
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        println!("CURRENT {:?}", cursor.current());

        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let account = Account { nonce: 2, ..Default::default() };
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let mut dup_cursor = tx.cursor_dup_write::<PlainStorageState>().unwrap();
        let subkey = B256::random();

        let value = U256::from(1);
        let entry1 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, entry1).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey), Ok(Some(entry1)));

        let value = U256::from(2);
        let entry2 = StorageEntry { key: subkey, value };
        dup_cursor.upsert(key, entry2).expect(ERROR_UPSERT);
        assert_eq!(dup_cursor.seek_by_key_subkey(key, subkey), Ok(Some(entry1)));
        assert_eq!(dup_cursor.next_dup_val(), Ok(Some(entry2)));
    }

    #[test]
    fn db_cursor_dupsort_append() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

        let transition_id = 2;

        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSets>().unwrap();
        vec![0, 1, 3, 4, 5]
            .into_iter()
            .try_for_each(|val| {
                cursor.append(
                    transition_id,
                    AccountBeforeTx { address: Address::with_last_byte(val), info: None },
                )
            })
            .expect(ERROR_APPEND);
        tx.commit().expect(ERROR_COMMIT);

        // APPEND DUP & APPEND
        let subkey_to_append = 2;
        let tx = db.tx_mut().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_write::<AccountChangeSets>().unwrap();
        assert_eq!(
            cursor.append_dup(
                transition_id,
                AccountBeforeTx { address: Address::with_last_byte(subkey_to_append), info: None }
            ),
            Err(DatabaseWriteError {
                info: Error::KeyMismatch.into(),
                operation: DatabaseWriteOperation::CursorAppendDup,
                table_name: AccountChangeSets::NAME,
                key: transition_id.encode().into(),
            }
            .into())
        );
        assert_eq!(
            cursor.append(
                transition_id - 1,
                AccountBeforeTx { address: Address::with_last_byte(subkey_to_append), info: None }
            ),
            Err(DatabaseWriteError {
                info: Error::KeyMismatch.into(),
                operation: DatabaseWriteOperation::CursorAppend,
                table_name: AccountChangeSets::NAME,
                key: (transition_id - 1).encode().into(),
            }
            .into())
        );
        assert_eq!(
            cursor.append(
                transition_id,
                AccountBeforeTx { address: Address::with_last_byte(subkey_to_append), info: None }
            ),
            Ok(())
        );
    }

    #[test]
    fn db_closure_put_get() {
        let path = TempDir::new().expect(ERROR_TEMPDIR).into_path();

        let value = Account {
            nonce: 18446744073709551615,
            bytecode_hash: Some(B256::random()),
            balance: U256::MAX,
        };
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        {
            let env = create_test_db_with_path(DatabaseEnvKind::RW, &path);

            // PUT
            let result = env.update(|tx| {
                tx.put::<PlainAccountState>(key, value).expect(ERROR_PUT);
                200
            });
            assert_eq!(result.expect(ERROR_RETURN_VALUE), 200);
        }

        let env = DatabaseEnv::open(
            &path,
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
        let env = create_test_db(DatabaseEnvKind::RW);
        let key0 = Address::from_str("0xa1c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        let key2 = Address::from_str("0xa3c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        let key3 = Address::from_str("0xa4c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        let key4 = Address::from_str("0xa5c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // PUT (0,0)
        let value00 = StorageEntry::default();
        env.update(|tx| tx.put::<PlainStorageState>(key, value00).expect(ERROR_PUT)).unwrap();

        // // PUT (2,2)
        let value22 = StorageEntry { key: B256::with_last_byte(2), value: U256::from(2) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value22).expect(ERROR_PUT)).unwrap();

        // // PUT (1,1)
        let value11 = StorageEntry { key: B256::with_last_byte(1), value: U256::from(1) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value11).expect(ERROR_PUT)).unwrap();

        let value33 = StorageEntry { key: B256::with_last_byte(5), value: U256::from(3) };
        env.update(|tx| tx.put::<PlainStorageState>(key, value33).expect(ERROR_PUT)).unwrap();

        let value44 = StorageEntry { key: B256::with_last_byte(4), value: U256::from(4) };
        env.update(|tx| tx.put::<PlainStorageState>(key3, value44).expect(ERROR_PUT)).unwrap();

        let value55 = StorageEntry { key: B256::with_last_byte(7), value: U256::from(5) };
        env.update(|tx| tx.put::<PlainStorageState>(key3, value55).expect(ERROR_PUT)).unwrap();

        let value66 = StorageEntry { key: B256::with_last_byte(8), value: U256::from(6) };
        env.update(|tx| tx.put::<PlainStorageState>(key3, value66).expect(ERROR_PUT)).unwrap();

        let value77 = StorageEntry { key: B256::with_last_byte(9), value: U256::from(9) };
        let value99 = StorageEntry { key: B256::with_last_byte(9), value: U256::from(10) };
        let value88 = StorageEntry { key: B256::with_last_byte(7), value: U256::from(7) };

        // Iterate with cursor
        {
            let tx = env.tx_mut().expect(ERROR_INIT_TX);
            // let tx_mut = env.tx_mut().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            println!("CURRENT: {:?}", cursor.current());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());

            println!("SEEK KEY2: {:?}", cursor.seek(key2));
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("NEXT DUP: {:?}", cursor.next_dup());
            println!("CURRENT: {:?}", cursor.current());

            let mut cursor1 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            println!("CURRENT: {:?}", cursor1.current());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());
            println!("NEXT NO DUP: {:?}", cursor1.next_no_dup());

            println!("CURRENT: {:?}", cursor1.current());

            let mut cursor2 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            println!("CURRENT: {:?}", cursor2.current());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());
            println!("NEXT DUP VAL: {:?}", cursor2.next_dup_val());

            println!("CURRENT: {:?}", cursor2.current());

            let mut cursor3 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY HERE: {:?}",
                cursor3.seek_by_key_subkey(key4, B256::with_last_byte(1))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY HERE: {:?}",
                cursor3.seek_by_key_subkey(key0, B256::with_last_byte(1))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key2, B256::with_last_byte(1))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key2, B256::with_last_byte(0))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key3, B256::with_last_byte(1))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key3, B256::with_last_byte(5))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key3, B256::with_last_byte(7))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key3, B256::with_last_byte(8))
            );
            println!("CURRENT: {:?}", cursor3.current());
            println!(
                "SEEK BY KEY SUBKEY THIS: {:?}",
                cursor3.seek_by_key_subkey(key3, B256::with_last_byte(9))
            );
            println!("CURRENT: {:?}", cursor3.current());

            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key, B256::with_last_byte(8))
            );
            println!("CURRENT: {:?}", cursor3.current());

            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key, B256::with_last_byte(3))
            );
            println!("CURRENT: {:?}", cursor3.current());

            println!(
                "SEEK BY KEY SUBKEY: {:?}",
                cursor3.seek_by_key_subkey(key, B256::with_last_byte(4))
            );
            println!("CURRENT: {:?}", cursor3.current());

            let mut cursor4 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor4.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            let mut cursor5 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            println!("APPEND DUP: {:?}", cursor5.append_dup(key, value77));
            println!("CURRENT: {:?}", cursor5.current());

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor6.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            let mut cursor7 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            println!("APPEND DUP: {:?}", cursor7.append_dup(key3, value77));
            println!("CURRENT: {:?}", cursor7.current());

            println!("APPEND DUP: {:?}", cursor7.append_dup(key3, value99));
            println!("CURRENT: {:?}", cursor7.current());

            println!("APPEND DUP: {:?}", cursor7.append_dup(key3, value66));
            println!("CURRENT: {:?}", cursor7.current());

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor6.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            println!("APPEND DUP: {:?}", cursor7.append_dup(key3, value11));
            println!("CURRENT: {:?}", cursor7.current());

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor6.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            println!("APPEND DUP: {:?}", cursor6.append_dup(key3, value33));
            println!("CURRENT: {:?}", cursor6.current());

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor6.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            println!("APPEND DUP: {:?}", cursor6.append_dup(key2, value33));
            println!("CURRENT: {:?}", cursor6.current());

            let mut cursor6 = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            while let Some((key, value)) = cursor6.next().unwrap() {
                println!("NEXT Key: {}, Value: {:?}", key, value);
            }

            // let mut cursor5 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            // println!("DELETE CURRENT DUPLICATES: {:?}", cursor5.delete_current_duplicates());
            // println!("CURRENT: {:?}", cursor5.current());

            // println!("SEEK EXACT: {:?}", cursor4.seek_exact(key));

            // println!("DELETE CURRENT DUPLICATES: {:?}", cursor4.delete_current_duplicates());
            // println!("CURRENT: {:?}", cursor4.current());

            // let mut cursor5 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            // while let Some((key, value)) = cursor5.next().unwrap() {
            //     println!("NEXT Key: {}, Value: {:?}", key, value);
            // };

            // println!("DELETE CURRENT DUPLICATES: {:?}", cursor4.delete_current_duplicates());
            // println!("CURRENT: {:?}", cursor4.current());

            // println!("SEEK EXACT: {:?}", cursor4.seek_exact(key3));

            // println!("DELETE CURRENT DUPLICATES: {:?}", cursor4.delete_current_duplicates());
            // println!("CURRENT: {:?}", cursor4.current());

            // let mut cursor5 = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            // while let Some((key, value)) = cursor5.next().unwrap() {
            //     println!("NEXT Key: {}, Value: {:?}", key, value);
            // };
            // Notice that value11 and value22 have been ordered in the DB.
            // assert_eq!(Some(value00), cursor.next_dup_val().unwrap());
            // assert_eq!(Some(value11), cursor.next_dup_val().unwrap());
            // assert_eq!(Some(value22), cursor.next_dup_val().unwrap());
            // assert_eq!(Some(value33), cursor.next_dup_val().unwrap());
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
    fn db_iterate_over_all_dup_values() {
        let env = create_test_db(DatabaseEnvKind::RW);
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
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value11))), walker.next());
            // NOTE: Dup cursor does NOT iterates on all values but only on duplicated values of the
            // same key. assert_eq!(Ok(Some(value22.clone())), walker.next());
            assert_eq!(None, walker.next());
        }

        // Iterate by using `walk`
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();
            let first = cursor.first().unwrap().unwrap();
            let mut walker = cursor.walk(Some(first.0)).unwrap();
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value11))), walker.next());
            assert_eq!(Some(Ok((key2, value22))), walker.next());
        }
    }

    #[test]
    fn dup_value_with_same_subkey() {
        let env = create_test_db(DatabaseEnvKind::RW);
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
            assert_eq!(Some(Ok((key1, value00))), walker.next());
            assert_eq!(Some(Ok((key1, value01))), walker.next());
            assert_eq!(Some(Ok((key2, value22))), walker.next());
        }

        // seek_by_key_subkey
        {
            let tx = env.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_dup_read::<PlainStorageState>().unwrap();

            // NOTE: There are two values with same SubKey but only first one is shown
            assert_eq!(Ok(Some(value00)), cursor.seek_by_key_subkey(key1, value00.key));
            // key1 but value is greater than the one in the DB
            assert_eq!(Ok(None), cursor.seek_by_key_subkey(key1, value22.key));
        }
    }

    #[test]
    fn db_sharded_key() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
        let real_key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();

        for i in 1..5 {
            let key = ShardedKey::new(real_key, i * 100);
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
