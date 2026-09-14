//! Datadir ownership independent of database handles, so a simulated node can reopen cold.

use reth_chainspec::ChainSpec;
use reth_db::{mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::{
    database::Database,
    database_metrics::DatabaseMetrics,
    table::{DupSort, Encode, Table, TableImporter},
    transaction::{DbTx, DbTxMut},
};
use reth_provider::{
    providers::{RocksDBBuilder, StaticFileProviderBuilder},
    test_utils::MockNodeTypesWithDB,
    ProviderFactory,
};
use reth_storage_errors::db::DatabaseError;
use reth_storage_overlay::OverlayManager;
use reth_tasks::Runtime;
use std::{
    fmt,
    sync::{Arc, Weak},
};

/// A database operation at which the simulator can return an I/O error.
#[derive(Clone, Copy, Debug)]
pub(super) struct DatabaseOperation {
    pub(super) kind: &'static str,
    pub(super) table: Option<&'static str>,
}

/// Shared fault decision callback. An unarmed controller adds no decision points.
#[derive(Clone)]
pub(super) struct DatabaseFaults {
    decide: Arc<dyn Fn(DatabaseOperation) -> bool + Send + Sync>,
}

impl DatabaseFaults {
    pub(super) fn disabled() -> Self {
        Self::new(|_| false)
    }

    pub(super) fn new(decide: impl Fn(DatabaseOperation) -> bool + Send + Sync + 'static) -> Self {
        Self { decide: Arc::new(decide) }
    }

    fn check(&self, kind: &'static str, table: Option<&'static str>) -> Result<(), DatabaseError> {
        if (self.decide)(DatabaseOperation { kind, table }) {
            Err(DatabaseError::Other(format!(
                "deterministic database fault: {kind}{}",
                table.map_or(String::new(), |table| format!(" {table}"))
            )))
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for DatabaseFaults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DatabaseFaults")
    }
}

#[derive(Debug)]
pub(super) struct FaultDatabase<DB> {
    inner: DB,
    faults: DatabaseFaults,
}

impl<DB> FaultDatabase<DB> {
    const fn new(inner: DB, faults: DatabaseFaults) -> Self {
        Self { inner, faults }
    }
}

#[derive(Debug)]
pub(super) struct FaultTx<TX> {
    inner: TX,
    faults: DatabaseFaults,
}

impl<DB: Database> Database for FaultDatabase<DB> {
    type TX = FaultTx<DB::TX>;
    type TXMut = FaultTx<DB::TXMut>;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        self.faults.check("open-read-transaction", None)?;
        Ok(FaultTx { inner: self.inner.tx()?, faults: self.faults.clone() })
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        self.faults.check("open-write-transaction", None)?;
        Ok(FaultTx { inner: self.inner.tx_mut()?, faults: self.faults.clone() })
    }

    fn path(&self) -> std::path::PathBuf {
        self.inner.path()
    }

    fn oldest_reader_txnid(&self) -> Option<u64> {
        self.inner.oldest_reader_txnid()
    }

    fn last_txnid(&self) -> Option<u64> {
        self.inner.last_txnid()
    }
}

impl<DB: DatabaseMetrics> DatabaseMetrics for FaultDatabase<DB> {
    fn report_metrics(&self) {
        self.inner.report_metrics();
    }

    fn gauge_metrics(&self) -> Vec<(&'static str, f64, Vec<metrics::Label>)> {
        self.inner.gauge_metrics()
    }

    fn counter_metrics(&self) -> Vec<(&'static str, u64, Vec<metrics::Label>)> {
        self.inner.counter_metrics()
    }

    fn histogram_metrics(&self) -> Vec<(&'static str, f64, Vec<metrics::Label>)> {
        self.inner.histogram_metrics()
    }
}

impl<TX: DbTx> DbTx for FaultTx<TX> {
    type Cursor<T: Table> = TX::Cursor<T>;
    type DupCursor<T: DupSort> = TX::DupCursor<T>;

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        self.faults.check("get", Some(T::NAME))?;
        self.inner.get::<T>(key)
    }

    fn get_by_encoded_key<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
    ) -> Result<Option<T::Value>, DatabaseError> {
        self.faults.check("get-encoded", Some(T::NAME))?;
        self.inner.get_by_encoded_key::<T>(key)
    }

    fn commit(self) -> Result<(), DatabaseError> {
        self.faults.check("commit", None)?;
        self.inner.commit()
    }

    fn abort(self) {
        self.inner.abort();
    }

    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError> {
        self.faults.check("open-read-cursor", Some(T::NAME))?;
        self.inner.cursor_read::<T>()
    }

    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError> {
        self.faults.check("open-read-dup-cursor", Some(T::NAME))?;
        self.inner.cursor_dup_read::<T>()
    }

    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        self.faults.check("entries", Some(T::NAME))?;
        self.inner.entries::<T>()
    }

    fn disable_long_read_transaction_safety(&mut self) {
        self.inner.disable_long_read_transaction_safety();
    }
}

impl<TX: DbTxMut> DbTxMut for FaultTx<TX> {
    type CursorMut<T: Table> = TX::CursorMut<T>;
    type DupCursorMut<T: DupSort> = TX::DupCursorMut<T>;

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        self.faults.check("put", Some(T::NAME))?;
        self.inner.put::<T>(key, value)
    }

    fn append<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        self.faults.check("append", Some(T::NAME))?;
        self.inner.append::<T>(key, value)
    }

    fn delete<T: Table>(
        &self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        self.faults.check("delete", Some(T::NAME))?;
        self.inner.delete::<T>(key, value)
    }

    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        self.faults.check("clear", Some(T::NAME))?;
        self.inner.clear::<T>()
    }

    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError> {
        self.faults.check("open-write-cursor", Some(T::NAME))?;
        self.inner.cursor_write::<T>()
    }

    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError> {
        self.faults.check("open-write-dup-cursor", Some(T::NAME))?;
        self.inner.cursor_dup_write::<T>()
    }
}

impl<TX: TableImporter> TableImporter for FaultTx<TX> {}

/// Owns the datadir across launches while keeping no live native database handle.
pub(super) struct NodeStorage {
    directory: tempfile::TempDir,
    chain: Arc<ChainSpec>,
    last_database: Weak<DatabaseEnv>,
    faults: DatabaseFaults,
}

impl NodeStorage {
    pub(super) fn new(chain: Arc<ChainSpec>, faults: DatabaseFaults) -> Self {
        Self { directory: tempfile::tempdir().unwrap(), chain, last_database: Weak::new(), faults }
    }

    /// Opening twice without releasing the previous engine/provider is a harness error.
    pub(super) fn open(&mut self, overlay: OverlayManager, runtime: Runtime) -> Factory {
        assert!(self.last_database.upgrade().is_none(), "node still holds its database open");
        let database = Arc::new(
            reth_db::init_db(self.directory.path().join("db"), DatabaseArguments::test()).unwrap(),
        );
        self.last_database = Arc::downgrade(&database);
        reth_fs_util::create_dir_all(self.directory.path().join("static_files")).unwrap();
        ProviderFactory::new(
            Arc::new(FaultDatabase::new(database, self.faults.clone())),
            self.chain.clone(),
            StaticFileProviderBuilder::read_write(self.directory.path().join("static_files"))
                .with_genesis_block_number(self.chain.genesis.number.unwrap_or_default())
                .build()
                .unwrap(),
            RocksDBBuilder::new(self.directory.path().join("rocksdb"))
                .with_default_tables()
                .build()
                .unwrap(),
            runtime,
        )
        .unwrap()
        .with_overlay_manager(overlay)
    }

    pub(super) fn is_open(&self) -> bool {
        self.last_database.upgrade().is_some()
    }
}

pub(super) type NodeTypes = MockNodeTypesWithDB<Arc<FaultDatabase<Arc<DatabaseEnv>>>>;
pub(super) type Factory = ProviderFactory<NodeTypes>;
