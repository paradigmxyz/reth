//! Module that interacts with MDBX.

use crate::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetadataValue, DatabaseMetrics},
    metrics::DatabaseEnvMetrics,
    tables::{TableType, Tables},
    utils::default_page_size,
    DatabaseError,
};
use eyre::Context;
use metrics::{gauge, Label};
use once_cell::sync::Lazy;
use reth_interfaces::db::LogLevel;
use reth_libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, Geometry, MaxReadTransactionDuration, Mode,
    PageSize, SyncMode, RO, RW,
};
use reth_tracing::tracing::error;
use std::{ops::Deref, path::Path, sync::Arc};
use tx::Tx;

pub mod cursor;
pub mod tx;

const GIGABYTE: usize = 1024 * 1024 * 1024;
const TERABYTE: usize = GIGABYTE * 1024;

/// MDBX allows up to 32767 readers (`MDBX_READERS_LIMIT`), but we limit it to slightly below that
const DEFAULT_MAX_READERS: u64 = 32_000;

/// Space that a read-only transaction can occupy until the warning is emitted.
/// See [reth_libmdbx::EnvironmentBuilder::set_handle_slow_readers] for more information.
#[cfg(not(windows))]
const MAX_SAFE_READER_SPACE: usize = 10 * GIGABYTE;

#[cfg(not(windows))]
static PROCESS_ID: Lazy<u32> = Lazy::new(|| {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }

    #[cfg(not(unix))]
    {
        std::process::id()
    }
});

/// Environment used when opening a MDBX environment. RO/RW.
#[derive(Debug)]
pub enum DatabaseEnvKind {
    /// Read-only MDBX environment.
    RO,
    /// Read-write MDBX environment.
    RW,
}

/// Arguments for database initialization.
#[derive(Debug, Default, Clone, Copy)]
pub struct DatabaseArguments {
    /// Database log level. If [None], the default value is used.
    log_level: Option<LogLevel>,
    /// Maximum duration of a read transaction. If [None], the default value is used.
    max_read_transaction_duration: Option<MaxReadTransactionDuration>,
}

impl DatabaseArguments {
    /// Set the log level.
    pub fn log_level(mut self, log_level: Option<LogLevel>) -> Self {
        self.log_level = log_level;
        self
    }

    /// Set the maximum duration of a read transaction.
    pub fn max_read_transaction_duration(
        mut self,
        max_read_transaction_duration: Option<MaxReadTransactionDuration>,
    ) -> Self {
        self.max_read_transaction_duration = max_read_transaction_duration;
        self
    }
}

/// Wrapper for the libmdbx environment: [Environment]
#[derive(Debug)]
pub struct DatabaseEnv {
    /// Libmdbx-sys environment.
    inner: Environment,
    /// Cache for metric handles. If `None`, metrics are not recorded.
    metrics: Option<Arc<DatabaseEnvMetrics>>,
}

impl Database for DatabaseEnv {
    type TX = tx::Tx<RO>;
    type TXMut = tx::Tx<RW>;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        Ok(Tx::new_with_metrics(
            self.inner.begin_ro_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.metrics.as_ref().cloned(),
        ))
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        Ok(Tx::new_with_metrics(
            self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?,
            self.metrics.as_ref().cloned(),
        ))
    }
}

impl DatabaseMetrics for DatabaseEnv {
    fn report_metrics(&self) {
        for (name, value, labels) in self.gauge_metrics() {
            gauge!(name, value, labels);
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
    ) -> Result<DatabaseEnv, DatabaseError> {
        let mut inner_env = Environment::builder();

        let mode = match kind {
            DatabaseEnvKind::RO => Mode::ReadOnly,
            DatabaseEnvKind::RW => {
                // enable writemap mode in RW mode
                inner_env.write_map();
                Mode::ReadWrite { sync_mode: SyncMode::Durable }
            }
        };

        inner_env.set_max_dbs(Tables::ALL.len());
        inner_env.set_geometry(Geometry {
            // Maximum database size of 4 terabytes
            size: Some(0..(4 * TERABYTE)),
            // We grow the database in increments of 4 gigabytes
            growth_step: Some(4 * GIGABYTE as isize),
            // The database never shrinks
            shrink_threshold: None,
            page_size: Some(PageSize::Set(default_page_size())),
        });
        #[cfg(not(windows))]
        {
            let _ = *PROCESS_ID; // Initialize the process ID at the time of environment opening
            inner_env.set_handle_slow_readers(
                |process_id: u32, thread_id: u32, read_txn_id: u64, gap: usize, space: usize, retry: isize| {
                    if space > MAX_SAFE_READER_SPACE {
                        let message = if process_id == *PROCESS_ID {
                            "Current process has a long-lived database transaction that grows the database file."
                        } else {
                            "External process has a long-lived database transaction that grows the database file. Use shorter-lived read transactions or shut down the node."
                        };
                        reth_tracing::tracing::warn!(
                            target: "storage::db::mdbx",
                            ?process_id,
                            ?thread_id,
                            ?read_txn_id,
                            ?gap,
                            ?space,
                            ?retry,
                            message
                        )
                    }

                    reth_libmdbx::HandleSlowReadersReturnCode::ProceedWithoutKillingReader
                });
        }
        inner_env.set_flags(EnvironmentFlags {
            mode,
            // We disable readahead because it improves performance for linear scans, but
            // worsens it for random access (which is our access pattern outside of sync)
            no_rdahead: true,
            coalesce: true,
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

        let env = DatabaseEnv {
            inner: inner_env.open(path).map_err(|e| DatabaseError::Open(e.into()))?,
            metrics: None,
        };

        Ok(env)
    }

    /// Enables metrics on the database.
    pub fn with_metrics(mut self) -> Self {
        self.metrics = Some(DatabaseEnvMetrics::new().into());
        self
    }

    /// Creates all the defined tables, if necessary.
    pub fn create_tables(&self) -> Result<(), DatabaseError> {
        let tx = self.inner.begin_rw_txn().map_err(|e| DatabaseError::InitTx(e.into()))?;

        for table in Tables::ALL {
            let flags = match table.table_type() {
                TableType::Table => DatabaseFlags::default(),
                TableType::DupSort => DatabaseFlags::DUP_SORT,
            };

            tx.create_db(Some(table.name()), flags)
                .map_err(|e| DatabaseError::CreateTable(e.into()))?;
        }

        tx.commit().map_err(|e| DatabaseError::Commit(e.into()))?;

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
        abstraction::table::{Encode, Table},
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, ReverseWalker, Walker},
        models::{AccountBeforeTx, ShardedKey},
        tables::{AccountHistory, CanonicalHeaders, Headers, PlainAccountState, PlainStorageState},
        test_utils::*,
        transaction::{DbTx, DbTxMut},
        AccountChangeSet,
    };
    use reth_interfaces::db::{DatabaseWriteError, DatabaseWriteOperation};
    use reth_libmdbx::Error;
    use reth_primitives::{Account, Address, Header, IntegerList, StorageEntry, B256, U256};
    use std::str::FromStr;
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
        let env =
            DatabaseEnv::open(path, kind, DatabaseArguments::default()).expect(ERROR_DB_CREATION);
        env.create_tables().expect(ERROR_TABLE_CREATION);
        env
    }

    const ERROR_DB_CREATION: &str = "Not able to create the mdbx file.";
    const ERROR_PUT: &str = "Not able to insert value into table.";
    const ERROR_APPEND: &str = "Not able to append the value to the table.";
    const ERROR_UPSERT: &str = "Not able to upsert the value to the table.";
    const ERROR_GET: &str = "Not able to get value from table.";
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
        assert!(result.expect(ERROR_RETURN_VALUE) == value);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_cursor_walk() {
        let env = create_test_db(DatabaseEnvKind::RW);

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
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(0, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address0, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address1, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(1, AccountBeforeTx { address: address2, info: None })
            .expect(ERROR_PUT);
        tx.put::<AccountChangeSet>(2, AccountBeforeTx { address: address0, info: None }) // <- should not be returned by the walker
            .expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        let tx = db.tx().expect(ERROR_INIT_TX);
        let mut cursor = tx.cursor_read::<AccountChangeSet>().unwrap();

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

        // Seek exact
        let exact = cursor.seek_exact(missing_key).unwrap();
        assert_eq!(exact, None);
        assert_eq!(cursor.current(), Ok(Some((missing_key + 1, B256::ZERO))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 1, B256::ZERO))));
        assert_eq!(cursor.prev(), Ok(Some((missing_key - 2, B256::ZERO))));
    }

    #[test]
    fn db_cursor_insert() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);

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
        assert_eq!(cursor.insert(key_to_insert, B256::ZERO), Ok(()));
        assert_eq!(cursor.current(), Ok(Some((key_to_insert, B256::ZERO))));

        // INSERT (failure)
        assert_eq!(
            cursor.insert(key_to_insert, B256::ZERO),
            Err(DatabaseWriteError {
                info: Error::KeyExist.into(),
                operation: DatabaseWriteOperation::CursorInsert,
                table_name: CanonicalHeaders::NAME,
                key: key_to_insert.encode().into(),
            }
            .into())
        );
        assert_eq!(cursor.current(), Ok(Some((key_to_insert, B256::ZERO))));

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
        let mut cursor = tx.cursor_write::<PlainAccountState>().unwrap();

        assert!(cursor.insert(key1, Account::default()).is_ok());
        assert!(cursor.insert(key2, Account::default()).is_ok());
        assert!(cursor.insert(key3, Account::default()).is_ok());

        // Seek & delete key2
        cursor.seek_exact(key2).unwrap();
        assert_eq!(cursor.delete_current(), Ok(()));
        assert_eq!(cursor.seek_exact(key2), Ok(None));

        // Seek & delete key2 again
        assert_eq!(cursor.seek_exact(key2), Ok(None));
        assert_eq!(cursor.delete_current(), Ok(()));
        // Assert that key1 is still there
        assert_eq!(cursor.seek_exact(key1), Ok(Some((key1, Account::default()))));
        // Assert that key3 was deleted
        assert_eq!(cursor.seek_exact(key3), Ok(None));
    }

    #[test]
    fn db_cursor_insert_wherever_cursor_is() {
        let db: Arc<DatabaseEnv> = create_test_db(DatabaseEnvKind::RW);
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
        assert_eq!(cursor.current(), Ok(Some((9, B256::ZERO))));

        for pos in (2..=8).step_by(2) {
            assert_eq!(cursor.insert(pos, B256::ZERO), Ok(()));
            assert_eq!(cursor.current(), Ok(Some((pos, B256::ZERO))));
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
        assert_eq!(cursor.append(key_to_append, B256::ZERO), Ok(()));
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

        let account = Account::default();
        cursor.upsert(key, account).expect(ERROR_UPSERT);
        assert_eq!(cursor.seek_exact(key), Ok(Some((key, account))));

        let account = Account { nonce: 1, ..Default::default() };
        cursor.upsert(key, account).expect(ERROR_UPSERT);
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
        let mut cursor = tx.cursor_write::<AccountChangeSet>().unwrap();
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
        let mut cursor = tx.cursor_write::<AccountChangeSet>().unwrap();
        assert_eq!(
            cursor.append_dup(
                transition_id,
                AccountBeforeTx { address: Address::with_last_byte(subkey_to_append), info: None }
            ),
            Err(DatabaseWriteError {
                info: Error::KeyMismatch.into(),
                operation: DatabaseWriteOperation::CursorAppendDup,
                table_name: AccountChangeSet::NAME,
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
                table_name: AccountChangeSet::NAME,
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

        let env = DatabaseEnv::open(&path, DatabaseEnvKind::RO, Default::default())
            .expect(ERROR_DB_CREATION);

        // GET
        let result =
            env.view(|tx| tx.get::<PlainAccountState>(key).expect(ERROR_GET)).expect(ERROR_GET);

        assert_eq!(result, Some(value))
    }

    #[test]
    fn db_dup_sort() {
        let env = create_test_db(DatabaseEnvKind::RW);
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
            assert!(Some(value00) == cursor.next_dup_val().unwrap());
            assert!(Some(value11) == cursor.next_dup_val().unwrap());
            assert!(Some(value22) == cursor.next_dup_val().unwrap());
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
            let list: IntegerList = vec![i * 100u64].into();

            db.update(|tx| tx.put::<AccountHistory>(key.clone(), list.clone()).expect("")).unwrap();
        }

        // Seek value with non existing key.
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountHistory>().unwrap();

            // It will seek the one greater or equal to the query. Since we have `Address | 100`,
            // `Address | 200` in the database and we're querying `Address | 150` it will return us
            // `Address | 200`.
            let mut walker = cursor.walk(Some(ShardedKey::new(real_key, 150))).unwrap();
            let (key, list) = walker
                .next()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 200), key);
            let list200: IntegerList = vec![200u64].into();
            assert_eq!(list200, list);
        }
        // Seek greatest index
        {
            let tx = db.tx().expect(ERROR_INIT_TX);
            let mut cursor = tx.cursor_read::<AccountHistory>().unwrap();

            // It will seek the MAX value of transition index and try to use prev to get first
            // biggers.
            let _unknown = cursor.seek_exact(ShardedKey::new(real_key, u64::MAX)).unwrap();
            let (key, list) = cursor
                .prev()
                .expect("element should exist.")
                .expect("should be able to retrieve it.");

            assert_eq!(ShardedKey::new(real_key, 400), key);
            let list400: IntegerList = vec![400u64].into();
            assert_eq!(list400, list);
        }
    }
}
