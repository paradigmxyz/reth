use super::metrics::{RocksDBMetrics, RocksDBOperation, ROCKSDB_TABLES};
use crate::providers::{compute_history_rank, needs_prev_shard_check, HistoryInfo};
use alloy_consensus::transaction::TxHashRef;
use alloy_primitives::{
    map::{AddressMap, HashMap},
    Address, BlockNumber, TxNumber, B256,
};
use itertools::Itertools;
use metrics::Label;
use parking_lot::Mutex;
use reth_chain_state::ExecutedBlock;
use reth_db_api::{
    database_metrics::DatabaseMetrics,
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey, ShardedKey,
        StorageSettings,
    },
    table::{Compress, Decode, Decompress, Encode, Table},
    tables, BlockNumberList, DatabaseError,
};
use reth_primitives_traits::{BlockBody as _, FastInstant as Instant};
use reth_prune_types::PruneMode;
use reth_storage_errors::{
    db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation, LogLevel},
    provider::{ProviderError, ProviderResult},
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, CompactionPri, DBCompressionType,
    DBRawIteratorWithThreadMode, IteratorMode, OptimisticTransactionDB,
    OptimisticTransactionOptions, Options, Transaction, WriteBatchWithTransaction, WriteOptions,
    DB,
};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::instrument;

/// Pending `RocksDB` batches type alias.
pub(crate) type PendingRocksDBBatches = Arc<Mutex<Vec<WriteBatchWithTransaction<true>>>>;

/// Raw key-value result from a `RocksDB` iterator.
type RawKVResult = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>;

/// Statistics for a single `RocksDB` table (column family).
#[derive(Debug, Clone)]
pub struct RocksDBTableStats {
    /// Size of SST files on disk in bytes.
    pub sst_size_bytes: u64,
    /// Size of memtables in memory in bytes.
    pub memtable_size_bytes: u64,
    /// Name of the table/column family.
    pub name: String,
    /// Estimated number of keys in the table.
    pub estimated_num_keys: u64,
    /// Estimated size of live data in bytes (SST files + memtables).
    pub estimated_size_bytes: u64,
    /// Estimated bytes pending compaction (reclaimable space).
    pub pending_compaction_bytes: u64,
}

/// Database-level statistics for `RocksDB`.
///
/// Contains both per-table statistics and DB-level metrics like WAL size.
#[derive(Debug, Clone)]
pub struct RocksDBStats {
    /// Statistics for each table (column family).
    pub tables: Vec<RocksDBTableStats>,
    /// Total size of WAL (Write-Ahead Log) files in bytes.
    ///
    /// WAL is shared across all tables and not included in per-table metrics.
    pub wal_size_bytes: u64,
}

/// Context for `RocksDB` block writes.
#[derive(Clone)]
pub(crate) struct RocksDBWriteCtx {
    /// The first block number being written.
    pub first_block_number: BlockNumber,
    /// The prune mode for transaction lookup, if any.
    pub prune_tx_lookup: Option<PruneMode>,
    /// Storage settings determining what goes to `RocksDB`.
    pub storage_settings: StorageSettings,
    /// Pending batches to push to after writing.
    pub pending_batches: PendingRocksDBBatches,
}

impl fmt::Debug for RocksDBWriteCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBWriteCtx")
            .field("first_block_number", &self.first_block_number)
            .field("prune_tx_lookup", &self.prune_tx_lookup)
            .field("storage_settings", &self.storage_settings)
            .field("pending_batches", &"<pending batches>")
            .finish()
    }
}

/// Default cache size for `RocksDB` block cache (128 MB).
const DEFAULT_CACHE_SIZE: usize = 128 << 20;

/// Default block size for `RocksDB` tables (16 KB).
const DEFAULT_BLOCK_SIZE: usize = 16 * 1024;

/// Default max background jobs for `RocksDB` compaction and flushing.
const DEFAULT_MAX_BACKGROUND_JOBS: i32 = 6;

/// Default max open file descriptors for `RocksDB`.
///
/// Caps the number of SST file handles `RocksDB` keeps open simultaneously.
/// Set to 512 to stay within the common default OS `ulimit -n` of 1024,
/// leaving headroom for MDBX, static files, and other I/O.
/// `RocksDB` uses an internal table cache and re-opens files on demand,
/// so this has negligible performance impact on read-heavy workloads.
const DEFAULT_MAX_OPEN_FILES: i32 = 512;

/// Default bytes per sync for `RocksDB` WAL writes (1 MB).
const DEFAULT_BYTES_PER_SYNC: u64 = 1_048_576;

/// Default write buffer size for `RocksDB` memtables (128 MB).
///
/// Larger memtables reduce flush frequency during burst writes, providing more consistent
/// tail latency. Benchmarks showed 128 MB reduces p99 latency variance by ~80% compared
/// to 64 MB default, with negligible impact on mean throughput.
const DEFAULT_WRITE_BUFFER_SIZE: usize = 128 << 20;

/// Default buffer capacity for compression in batches.
/// 4 KiB matches common block/page sizes and comfortably holds typical history values,
/// reducing the first few reallocations without over-allocating.
const DEFAULT_COMPRESS_BUF_CAPACITY: usize = 4096;

/// Default auto-commit threshold for batch writes (4 GiB).
///
/// When a batch exceeds this size, it is automatically committed to prevent OOM
/// during large bulk writes. The consistency check on startup heals any crash
/// that occurs between auto-commits.
const DEFAULT_AUTO_COMMIT_THRESHOLD: usize = 4 * 1024 * 1024 * 1024;

/// Builder for [`RocksDBProvider`].
pub struct RocksDBBuilder {
    path: PathBuf,
    column_families: Vec<String>,
    enable_metrics: bool,
    enable_statistics: bool,
    log_level: rocksdb::LogLevel,
    block_cache: Cache,
    read_only: bool,
}

impl fmt::Debug for RocksDBBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBBuilder")
            .field("path", &self.path)
            .field("column_families", &self.column_families)
            .field("enable_metrics", &self.enable_metrics)
            .finish()
    }
}

impl RocksDBBuilder {
    /// Creates a new builder with optimized default options.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let cache = Cache::new_lru_cache(DEFAULT_CACHE_SIZE);
        Self {
            path: path.as_ref().to_path_buf(),
            column_families: Vec::new(),
            enable_metrics: false,
            enable_statistics: false,
            log_level: rocksdb::LogLevel::Info,
            block_cache: cache,
            read_only: false,
        }
    }

    /// Creates default table options with shared block cache.
    fn default_table_options(cache: &Cache) -> BlockBasedOptions {
        let mut table_options = BlockBasedOptions::default();
        table_options.set_block_size(DEFAULT_BLOCK_SIZE);
        table_options.set_cache_index_and_filter_blocks(true);
        table_options.set_pin_l0_filter_and_index_blocks_in_cache(true);
        // Shared block cache for all column families.
        table_options.set_block_cache(cache);
        table_options
    }

    /// Creates optimized `RocksDB` options per `RocksDB` wiki recommendations.
    fn default_options(
        log_level: rocksdb::LogLevel,
        cache: &Cache,
        enable_statistics: bool,
    ) -> Options {
        // Follow recommend tuning guide from RocksDB wiki, see https://github.com/facebook/rocksdb/wiki/Setup-Options-and-Basic-Tuning
        let table_options = Self::default_table_options(cache);

        let mut options = Options::default();
        options.set_block_based_table_factory(&table_options);
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        options.set_max_background_jobs(DEFAULT_MAX_BACKGROUND_JOBS);
        options.set_bytes_per_sync(DEFAULT_BYTES_PER_SYNC);

        options.set_bottommost_compression_type(DBCompressionType::Zstd);
        options.set_bottommost_zstd_max_train_bytes(0, true);
        options.set_compression_type(DBCompressionType::Lz4);
        options.set_compaction_pri(CompactionPri::MinOverlappingRatio);

        options.set_log_level(log_level);

        options.set_max_open_files(DEFAULT_MAX_OPEN_FILES);

        // Delete obsolete WAL files immediately after all column families have flushed.
        // Both set to 0 means "delete ASAP, no archival".
        options.set_wal_ttl_seconds(0);
        options.set_wal_size_limit_mb(0);

        // Statistics can view from RocksDB log file
        if enable_statistics {
            options.enable_statistics();
        }

        options
    }

    /// Creates optimized column family options.
    fn default_column_family_options(cache: &Cache) -> Options {
        // Follow recommend tuning guide from RocksDB wiki, see https://github.com/facebook/rocksdb/wiki/Setup-Options-and-Basic-Tuning
        let table_options = Self::default_table_options(cache);

        let mut cf_options = Options::default();
        cf_options.set_block_based_table_factory(&table_options);
        cf_options.set_level_compaction_dynamic_level_bytes(true);
        // Recommend to use Zstd for bottommost compression and Lz4 for other levels, see https://github.com/facebook/rocksdb/wiki/Compression#configuration
        cf_options.set_compression_type(DBCompressionType::Lz4);
        cf_options.set_bottommost_compression_type(DBCompressionType::Zstd);
        // Only use Zstd compression, disable dictionary training
        cf_options.set_bottommost_zstd_max_train_bytes(0, true);
        cf_options.set_write_buffer_size(DEFAULT_WRITE_BUFFER_SIZE);

        cf_options
    }

    /// Creates optimized column family options for `TransactionHashNumbers`.
    ///
    /// This table stores `B256 -> TxNumber` mappings where:
    /// - Keys are incompressible 32-byte hashes (compression wastes CPU for zero benefit)
    /// - Values are varint-encoded `u64` (a few bytes - too small to benefit from compression)
    /// - Every lookup expects a hit (bloom filters only help when checking non-existent keys)
    fn tx_hash_numbers_column_family_options(cache: &Cache) -> Options {
        let mut table_options = BlockBasedOptions::default();
        table_options.set_block_size(DEFAULT_BLOCK_SIZE);
        table_options.set_cache_index_and_filter_blocks(true);
        table_options.set_pin_l0_filter_and_index_blocks_in_cache(true);
        table_options.set_block_cache(cache);
        // Disable bloom filter: every lookup expects a hit, so bloom filters provide no benefit
        // and waste memory

        let mut cf_options = Options::default();
        cf_options.set_block_based_table_factory(&table_options);
        cf_options.set_level_compaction_dynamic_level_bytes(true);
        // Disable compression: B256 keys are incompressible hashes, TxNumber values are
        // varint-encoded u64 (a few bytes). Compression wastes CPU cycles for zero space savings.
        cf_options.set_compression_type(DBCompressionType::None);
        cf_options.set_bottommost_compression_type(DBCompressionType::None);

        cf_options
    }

    /// Adds a column family for a specific table type.
    pub fn with_table<T: Table>(mut self) -> Self {
        self.column_families.push(T::NAME.to_string());
        self
    }

    /// Registers the default tables used by reth for `RocksDB` storage.
    ///
    /// This registers:
    /// - [`tables::TransactionHashNumbers`] - Transaction hash to number mapping
    /// - [`tables::AccountsHistory`] - Account history index
    /// - [`tables::StoragesHistory`] - Storage history index
    pub fn with_default_tables(self) -> Self {
        self.with_table::<tables::TransactionHashNumbers>()
            .with_table::<tables::AccountsHistory>()
            .with_table::<tables::StoragesHistory>()
    }

    /// Enables metrics.
    pub const fn with_metrics(mut self) -> Self {
        self.enable_metrics = true;
        self
    }

    /// Enables `RocksDB` internal statistics collection.
    pub const fn with_statistics(mut self) -> Self {
        self.enable_statistics = true;
        self
    }

    /// Sets the log level from `DatabaseArgs` configuration.
    pub const fn with_database_log_level(mut self, log_level: Option<LogLevel>) -> Self {
        if let Some(level) = log_level {
            self.log_level = convert_log_level(level);
        }
        self
    }

    /// Sets a custom block cache size.
    pub fn with_block_cache_size(mut self, capacity_bytes: usize) -> Self {
        self.block_cache = Cache::new_lru_cache(capacity_bytes);
        self
    }

    /// Sets read-only mode.
    ///
    /// Note: Write operations on a read-only provider will panic at runtime.
    pub const fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Builds the [`RocksDBProvider`].
    pub fn build(self) -> ProviderResult<RocksDBProvider> {
        let options =
            Self::default_options(self.log_level, &self.block_cache, self.enable_statistics);

        let cf_descriptors: Vec<ColumnFamilyDescriptor> = self
            .column_families
            .iter()
            .map(|name| {
                let cf_options = if name == tables::TransactionHashNumbers::NAME {
                    Self::tx_hash_numbers_column_family_options(&self.block_cache)
                } else {
                    Self::default_column_family_options(&self.block_cache)
                };
                ColumnFamilyDescriptor::new(name.clone(), cf_options)
            })
            .collect();

        let metrics = self.enable_metrics.then(RocksDBMetrics::default);

        if self.read_only {
            let db = DB::open_cf_descriptors_read_only(&options, &self.path, cf_descriptors, false)
                .map_err(|e| {
                    ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    }))
                })?;
            Ok(RocksDBProvider(Arc::new(RocksDBProviderInner::ReadOnly { db, metrics })))
        } else {
            // Use OptimisticTransactionDB for MDBX-like transaction semantics (read-your-writes,
            // rollback) OptimisticTransactionDB uses optimistic concurrency control (conflict
            // detection at commit) and is backed by DBCommon, giving us access to
            // cancel_all_background_work for clean shutdown.
            let db =
                OptimisticTransactionDB::open_cf_descriptors(&options, &self.path, cf_descriptors)
                    .map_err(|e| {
                        ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                            message: e.to_string().into(),
                            code: -1,
                        }))
                    })?;
            Ok(RocksDBProvider(Arc::new(RocksDBProviderInner::ReadWrite { db, metrics })))
        }
    }
}

/// Some types don't support compression (eg. B256), and we don't want to be copying them to the
/// allocated buffer when we can just use their reference.
macro_rules! compress_to_buf_or_ref {
    ($buf:expr, $value:expr) => {
        if let Some(value) = $value.uncompressable_ref() {
            Some(value)
        } else {
            $buf.clear();
            $value.compress_to_buf(&mut $buf);
            None
        }
    };
}

/// `RocksDB` provider for auxiliary storage layer beside main database MDBX.
#[derive(Debug)]
pub struct RocksDBProvider(Arc<RocksDBProviderInner>);

/// Inner state for `RocksDB` provider.
enum RocksDBProviderInner {
    /// Read-write mode using `OptimisticTransactionDB`.
    ReadWrite {
        /// `RocksDB` database instance with optimistic transaction support.
        db: OptimisticTransactionDB,
        /// Metrics latency & operations.
        metrics: Option<RocksDBMetrics>,
    },
    /// Read-only mode using `DB` opened with `open_cf_descriptors_read_only`.
    /// This doesn't acquire an exclusive lock, allowing concurrent reads.
    ReadOnly {
        /// Read-only `RocksDB` database instance.
        db: DB,
        /// Metrics latency & operations.
        metrics: Option<RocksDBMetrics>,
    },
}

impl RocksDBProviderInner {
    /// Returns the metrics for this provider.
    const fn metrics(&self) -> Option<&RocksDBMetrics> {
        match self {
            Self::ReadWrite { metrics, .. } | Self::ReadOnly { metrics, .. } => metrics.as_ref(),
        }
    }

    /// Returns the read-write database, panicking if in read-only mode.
    fn db_rw(&self) -> &OptimisticTransactionDB {
        match self {
            Self::ReadWrite { db, .. } => db,
            Self::ReadOnly { .. } => {
                panic!("Cannot perform write operation on read-only RocksDB provider")
            }
        }
    }

    /// Gets the column family handle for a table.
    fn cf_handle<T: Table>(&self) -> Result<&rocksdb::ColumnFamily, DatabaseError> {
        let cf = match self {
            Self::ReadWrite { db, .. } => db.cf_handle(T::NAME),
            Self::ReadOnly { db, .. } => db.cf_handle(T::NAME),
        };
        cf.ok_or_else(|| DatabaseError::Other(format!("Column family '{}' not found", T::NAME)))
    }

    /// Gets the column family handle for a table from the read-write database.
    ///
    /// # Panics
    /// Panics if in read-only mode.
    fn cf_handle_rw(&self, name: &str) -> Result<&rocksdb::ColumnFamily, DatabaseError> {
        self.db_rw()
            .cf_handle(name)
            .ok_or_else(|| DatabaseError::Other(format!("Column family '{}' not found", name)))
    }

    /// Gets a value from a column family.
    fn get_cf(
        &self,
        cf: &rocksdb::ColumnFamily,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Vec<u8>>, rocksdb::Error> {
        match self {
            Self::ReadWrite { db, .. } => db.get_cf(cf, key),
            Self::ReadOnly { db, .. } => db.get_cf(cf, key),
        }
    }

    /// Puts a value into a column family.
    fn put_cf(
        &self,
        cf: &rocksdb::ColumnFamily,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
    ) -> Result<(), rocksdb::Error> {
        self.db_rw().put_cf(cf, key, value)
    }

    /// Deletes a value from a column family.
    fn delete_cf(
        &self,
        cf: &rocksdb::ColumnFamily,
        key: impl AsRef<[u8]>,
    ) -> Result<(), rocksdb::Error> {
        self.db_rw().delete_cf(cf, key)
    }

    /// Deletes a range of values from a column family.
    fn delete_range_cf<K: AsRef<[u8]>>(
        &self,
        cf: &rocksdb::ColumnFamily,
        from: K,
        to: K,
    ) -> Result<(), rocksdb::Error> {
        self.db_rw().delete_range_cf(cf, from, to)
    }

    /// Returns an iterator over a column family.
    fn iterator_cf(
        &self,
        cf: &rocksdb::ColumnFamily,
        mode: IteratorMode<'_>,
    ) -> RocksDBIterEnum<'_> {
        match self {
            Self::ReadWrite { db, .. } => RocksDBIterEnum::ReadWrite(db.iterator_cf(cf, mode)),
            Self::ReadOnly { db, .. } => RocksDBIterEnum::ReadOnly(db.iterator_cf(cf, mode)),
        }
    }

    /// Returns a raw iterator over a column family.
    ///
    /// Unlike [`Self::iterator_cf`], raw iterators support `seek()` for efficient
    /// repositioning without creating a new iterator.
    fn raw_iterator_cf(&self, cf: &rocksdb::ColumnFamily) -> RocksDBRawIterEnum<'_> {
        match self {
            Self::ReadWrite { db, .. } => RocksDBRawIterEnum::ReadWrite(db.raw_iterator_cf(cf)),
            Self::ReadOnly { db, .. } => RocksDBRawIterEnum::ReadOnly(db.raw_iterator_cf(cf)),
        }
    }

    /// Returns the path to the database directory.
    fn path(&self) -> &Path {
        match self {
            Self::ReadWrite { db, .. } => db.path(),
            Self::ReadOnly { db, .. } => db.path(),
        }
    }

    /// Returns the total size of WAL (Write-Ahead Log) files in bytes.
    ///
    /// WAL files have a `.log` extension in the `RocksDB` directory.
    fn wal_size_bytes(&self) -> u64 {
        let path = self.path();

        match std::fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum(),
            Err(_) => 0,
        }
    }

    /// Returns statistics for all column families in the database.
    fn table_stats(&self) -> Vec<RocksDBTableStats> {
        let mut stats = Vec::new();

        macro_rules! collect_stats {
            ($db:expr) => {
                for cf_name in ROCKSDB_TABLES {
                    if let Some(cf) = $db.cf_handle(cf_name) {
                        let estimated_num_keys = $db
                            .property_int_value_cf(cf, rocksdb::properties::ESTIMATE_NUM_KEYS)
                            .ok()
                            .flatten()
                            .unwrap_or(0);

                        // SST files size (on-disk) + memtable size (in-memory)
                        let sst_size = $db
                            .property_int_value_cf(cf, rocksdb::properties::LIVE_SST_FILES_SIZE)
                            .ok()
                            .flatten()
                            .unwrap_or(0);

                        let memtable_size = $db
                            .property_int_value_cf(cf, rocksdb::properties::SIZE_ALL_MEM_TABLES)
                            .ok()
                            .flatten()
                            .unwrap_or(0);

                        let estimated_size_bytes = sst_size + memtable_size;

                        let pending_compaction_bytes = $db
                            .property_int_value_cf(
                                cf,
                                rocksdb::properties::ESTIMATE_PENDING_COMPACTION_BYTES,
                            )
                            .ok()
                            .flatten()
                            .unwrap_or(0);

                        stats.push(RocksDBTableStats {
                            sst_size_bytes: sst_size,
                            memtable_size_bytes: memtable_size,
                            name: cf_name.to_string(),
                            estimated_num_keys,
                            estimated_size_bytes,
                            pending_compaction_bytes,
                        });
                    }
                }
            };
        }

        match self {
            Self::ReadWrite { db, .. } => collect_stats!(db),
            Self::ReadOnly { db, .. } => collect_stats!(db),
        }

        stats
    }

    /// Returns database-level statistics including per-table stats and WAL size.
    fn db_stats(&self) -> RocksDBStats {
        RocksDBStats { tables: self.table_stats(), wal_size_bytes: self.wal_size_bytes() }
    }
}

impl fmt::Debug for RocksDBProviderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadWrite { metrics, .. } => f
                .debug_struct("RocksDBProviderInner::ReadWrite")
                .field("db", &"<OptimisticTransactionDB>")
                .field("metrics", metrics)
                .finish(),
            Self::ReadOnly { metrics, .. } => f
                .debug_struct("RocksDBProviderInner::ReadOnly")
                .field("db", &"<DB (read-only)>")
                .field("metrics", metrics)
                .finish(),
        }
    }
}

impl Drop for RocksDBProviderInner {
    fn drop(&mut self) {
        match self {
            Self::ReadWrite { db, .. } => {
                // Flush all memtables if possible. If not, they will be rebuilt from the WAL on
                // restart
                if let Err(e) = db.flush_wal(true) {
                    tracing::warn!(target: "providers::rocksdb", ?e, "Failed to flush WAL on drop");
                }
                for cf_name in ROCKSDB_TABLES {
                    if let Some(cf) = db.cf_handle(cf_name) &&
                        let Err(e) = db.flush_cf(&cf)
                    {
                        tracing::warn!(target: "providers::rocksdb", cf = cf_name, ?e, "Failed to flush CF on drop");
                    }
                }
                db.cancel_all_background_work(true);
            }
            Self::ReadOnly { db, .. } => db.cancel_all_background_work(true),
        }
    }
}

impl Clone for RocksDBProvider {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl DatabaseMetrics for RocksDBProvider {
    fn gauge_metrics(&self) -> Vec<(&'static str, f64, Vec<Label>)> {
        let mut metrics = Vec::new();

        for stat in self.table_stats() {
            metrics.push((
                "rocksdb.table_size",
                stat.estimated_size_bytes as f64,
                vec![Label::new("table", stat.name.clone())],
            ));
            metrics.push((
                "rocksdb.table_entries",
                stat.estimated_num_keys as f64,
                vec![Label::new("table", stat.name.clone())],
            ));
            metrics.push((
                "rocksdb.pending_compaction_bytes",
                stat.pending_compaction_bytes as f64,
                vec![Label::new("table", stat.name.clone())],
            ));
            metrics.push((
                "rocksdb.sst_size",
                stat.sst_size_bytes as f64,
                vec![Label::new("table", stat.name.clone())],
            ));
            metrics.push((
                "rocksdb.memtable_size",
                stat.memtable_size_bytes as f64,
                vec![Label::new("table", stat.name)],
            ));
        }

        // WAL size (DB-level, shared across all tables)
        metrics.push(("rocksdb.wal_size", self.wal_size_bytes() as f64, vec![]));

        metrics
    }
}

impl RocksDBProvider {
    /// Creates a new `RocksDB` provider.
    pub fn new(path: impl AsRef<Path>) -> ProviderResult<Self> {
        RocksDBBuilder::new(path).build()
    }

    /// Creates a new `RocksDB` provider builder.
    pub fn builder(path: impl AsRef<Path>) -> RocksDBBuilder {
        RocksDBBuilder::new(path)
    }

    /// Returns `true` if a `RocksDB` database exists at the given path.
    ///
    /// Checks for the presence of the `CURRENT` file, which `RocksDB` creates
    /// when initializing a database.
    pub fn exists(path: impl AsRef<Path>) -> bool {
        path.as_ref().join("CURRENT").exists()
    }

    /// Returns `true` if this provider is in read-only mode.
    pub fn is_read_only(&self) -> bool {
        matches!(self.0.as_ref(), RocksDBProviderInner::ReadOnly { .. })
    }

    /// Creates a new transaction with MDBX-like semantics (read-your-writes, rollback).
    ///
    /// Note: With `OptimisticTransactionDB`, commits may fail if there are conflicts.
    /// Conflict detection happens at commit time, not at write time.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    pub fn tx(&self) -> RocksTx<'_> {
        let write_options = WriteOptions::default();
        let txn_options = OptimisticTransactionOptions::default();
        let inner = self.0.db_rw().transaction_opt(&write_options, &txn_options);
        RocksTx { inner, provider: self }
    }

    /// Creates a new batch for atomic writes.
    ///
    /// Use [`Self::write_batch`] for closure-based atomic writes.
    /// Use this method when the batch needs to be held by [`crate::EitherWriter`].
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode when attempting to commit.
    pub fn batch(&self) -> RocksDBBatch<'_> {
        RocksDBBatch {
            provider: self,
            inner: WriteBatchWithTransaction::<true>::default(),
            buf: Vec::with_capacity(DEFAULT_COMPRESS_BUF_CAPACITY),
            auto_commit_threshold: None,
        }
    }

    /// Creates a new batch with auto-commit enabled.
    ///
    /// When the batch size exceeds the threshold (4 GiB), the batch is automatically
    /// committed and reset. This prevents OOM during large bulk writes while maintaining
    /// crash-safety via the consistency check on startup.
    pub fn batch_with_auto_commit(&self) -> RocksDBBatch<'_> {
        RocksDBBatch {
            provider: self,
            inner: WriteBatchWithTransaction::<true>::default(),
            buf: Vec::with_capacity(DEFAULT_COMPRESS_BUF_CAPACITY),
            auto_commit_threshold: Some(DEFAULT_AUTO_COMMIT_THRESHOLD),
        }
    }

    /// Gets the column family handle for a table.
    fn get_cf_handle<T: Table>(&self) -> Result<&rocksdb::ColumnFamily, DatabaseError> {
        self.0.cf_handle::<T>()
    }

    /// Executes a function and records metrics with the given operation and table name.
    fn execute_with_operation_metric<R>(
        &self,
        operation: RocksDBOperation,
        table: &'static str,
        f: impl FnOnce(&Self) -> R,
    ) -> R {
        let start = self.0.metrics().map(|_| Instant::now());
        let res = f(self);

        if let (Some(start), Some(metrics)) = (start, self.0.metrics()) {
            metrics.record_operation(operation, table, start.elapsed());
        }

        res
    }

    /// Gets a value from the specified table.
    pub fn get<T: Table>(&self, key: T::Key) -> ProviderResult<Option<T::Value>> {
        self.get_encoded::<T>(&key.encode())
    }

    /// Gets a value from the specified table using pre-encoded key.
    pub fn get_encoded<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
    ) -> ProviderResult<Option<T::Value>> {
        self.execute_with_operation_metric(RocksDBOperation::Get, T::NAME, |this| {
            let result = this.0.get_cf(this.get_cf_handle::<T>()?, key.as_ref()).map_err(|e| {
                ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))
            })?;

            Ok(result.and_then(|value| T::Value::decompress(&value).ok()))
        })
    }

    /// Puts upsert a value into the specified table with the given key.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    pub fn put<T: Table>(&self, key: T::Key, value: &T::Value) -> ProviderResult<()> {
        let encoded_key = key.encode();
        self.put_encoded::<T>(&encoded_key, value)
    }

    /// Puts a value into the specified table using pre-encoded key.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    pub fn put_encoded<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
        value: &T::Value,
    ) -> ProviderResult<()> {
        self.execute_with_operation_metric(RocksDBOperation::Put, T::NAME, |this| {
            // for simplify the code, we need allocate buf here each time because `RocksDBProvider`
            // is thread safe if user want to avoid allocate buf each time, they can use
            // write_batch api
            let mut buf = Vec::new();
            let value_bytes = compress_to_buf_or_ref!(buf, value).unwrap_or(&buf);

            this.0.put_cf(this.get_cf_handle::<T>()?, key, value_bytes).map_err(|e| {
                ProviderError::Database(DatabaseError::Write(Box::new(DatabaseWriteError {
                    info: DatabaseErrorInfo { message: e.to_string().into(), code: -1 },
                    operation: DatabaseWriteOperation::PutUpsert,
                    table_name: T::NAME,
                    key: key.as_ref().to_vec(),
                })))
            })
        })
    }

    /// Deletes a value from the specified table.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    pub fn delete<T: Table>(&self, key: T::Key) -> ProviderResult<()> {
        self.execute_with_operation_metric(RocksDBOperation::Delete, T::NAME, |this| {
            this.0.delete_cf(this.get_cf_handle::<T>()?, key.encode().as_ref()).map_err(|e| {
                ProviderError::Database(DatabaseError::Delete(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))
            })
        })
    }

    /// Clears all entries from the specified table.
    ///
    /// Uses `delete_range_cf` from empty key to a max key (256 bytes of 0xFF).
    /// This end key must exceed the maximum encoded key size for any table.
    /// Current max is ~60 bytes (`StorageShardedKey` = 20 + 32 + 8).
    pub fn clear<T: Table>(&self) -> ProviderResult<()> {
        let cf = self.get_cf_handle::<T>()?;

        self.0.delete_range_cf(cf, &[] as &[u8], &[0xFF; 256]).map_err(|e| {
            ProviderError::Database(DatabaseError::Delete(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })?;

        Ok(())
    }

    /// Retrieves the first or last entry from a table based on the iterator mode.
    fn get_boundary<T: Table>(
        &self,
        mode: IteratorMode<'_>,
    ) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.execute_with_operation_metric(RocksDBOperation::Get, T::NAME, |this| {
            let cf = this.get_cf_handle::<T>()?;
            let mut iter = this.0.iterator_cf(cf, mode);

            match iter.next() {
                Some(Ok((key_bytes, value_bytes))) => {
                    let key = <T::Key as reth_db_api::table::Decode>::decode(&key_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
                    let value = T::Value::decompress(&value_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
                    Ok(Some((key, value)))
                }
                Some(Err(e)) => {
                    Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    })))
                }
                None => Ok(None),
            }
        })
    }

    /// Gets the first (smallest key) entry from the specified table.
    #[inline]
    pub fn first<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.get_boundary::<T>(IteratorMode::Start)
    }

    /// Gets the last (largest key) entry from the specified table.
    #[inline]
    pub fn last<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.get_boundary::<T>(IteratorMode::End)
    }

    /// Creates an iterator over all entries in the specified table.
    ///
    /// Returns decoded `(Key, Value)` pairs in key order.
    pub fn iter<T: Table>(&self) -> ProviderResult<RocksDBIter<'_, T>> {
        let cf = self.get_cf_handle::<T>()?;
        let iter = self.0.iterator_cf(cf, IteratorMode::Start);
        Ok(RocksDBIter { inner: iter, _marker: std::marker::PhantomData })
    }

    /// Returns statistics for all column families in the database.
    ///
    /// Returns a vector of (`table_name`, `estimated_keys`, `estimated_size_bytes`) tuples.
    pub fn table_stats(&self) -> Vec<RocksDBTableStats> {
        self.0.table_stats()
    }

    /// Returns the total size of WAL (Write-Ahead Log) files in bytes.
    ///
    /// This scans the `RocksDB` directory for `.log` files and sums their sizes.
    /// WAL files can be significant (e.g., 2.7GB observed) and are not included
    /// in `table_size`, `sst_size`, or `memtable_size` metrics.
    pub fn wal_size_bytes(&self) -> u64 {
        self.0.wal_size_bytes()
    }

    /// Returns database-level statistics including per-table stats and WAL size.
    ///
    /// This combines [`Self::table_stats`] and [`Self::wal_size_bytes`] into a single struct.
    pub fn db_stats(&self) -> RocksDBStats {
        self.0.db_stats()
    }

    /// Flushes pending writes for the specified tables to disk.
    ///
    /// This performs a flush of:
    /// 1. The column family memtables for the specified table names to SST files
    /// 2. The Write-Ahead Log (WAL) with sync
    ///
    /// After this call completes, all data for the specified tables is durably persisted to disk.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all, fields(tables = ?tables))]
    pub fn flush(&self, tables: &[&'static str]) -> ProviderResult<()> {
        let db = self.0.db_rw();

        for cf_name in tables {
            if let Some(cf) = db.cf_handle(cf_name) {
                db.flush_cf(&cf).map_err(|e| {
                    ProviderError::Database(DatabaseError::Write(Box::new(DatabaseWriteError {
                        info: DatabaseErrorInfo { message: e.to_string().into(), code: -1 },
                        operation: DatabaseWriteOperation::Flush,
                        table_name: cf_name,
                        key: Vec::new(),
                    })))
                })?;
            }
        }

        db.flush_wal(true).map_err(|e| {
            ProviderError::Database(DatabaseError::Write(Box::new(DatabaseWriteError {
                info: DatabaseErrorInfo { message: e.to_string().into(), code: -1 },
                operation: DatabaseWriteOperation::Flush,
                table_name: "WAL",
                key: Vec::new(),
            })))
        })?;

        Ok(())
    }

    /// Flushes and compacts all tables in `RocksDB`.
    ///
    /// This:
    /// 1. Flushes all column family memtables to SST files
    /// 2. Flushes the Write-Ahead Log (WAL) with sync
    /// 3. Triggers manual compaction on all column families to reclaim disk space
    ///
    /// Use this after large delete operations (like pruning) to reclaim disk space.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    pub fn flush_and_compact(&self) -> ProviderResult<()> {
        self.flush(ROCKSDB_TABLES)?;

        let db = self.0.db_rw();

        for cf_name in ROCKSDB_TABLES {
            if let Some(cf) = db.cf_handle(cf_name) {
                db.compact_range_cf(&cf, None::<&[u8]>, None::<&[u8]>);
            }
        }

        Ok(())
    }

    /// Creates a raw iterator over all entries in the specified table.
    ///
    /// Returns raw `(key_bytes, value_bytes)` pairs without decoding.
    pub fn raw_iter<T: Table>(&self) -> ProviderResult<RocksDBRawIter<'_>> {
        let cf = self.get_cf_handle::<T>()?;
        let iter = self.0.iterator_cf(cf, IteratorMode::Start);
        Ok(RocksDBRawIter { inner: iter })
    }

    /// Returns all account history shards for the given address in ascending key order.
    ///
    /// This is used for unwind operations where we need to scan all shards for an address
    /// and potentially delete or truncate them.
    pub fn account_history_shards(
        &self,
        address: Address,
    ) -> ProviderResult<Vec<(ShardedKey<Address>, BlockNumberList)>> {
        // Get the column family handle for the AccountsHistory table.
        let cf = self.get_cf_handle::<tables::AccountsHistory>()?;

        // Build a seek key starting at the first shard (highest_block_number = 0) for this address.
        // ShardedKey is (address, highest_block_number) so this positions us at the beginning.
        let start_key = ShardedKey::new(address, 0u64);
        let start_bytes = start_key.encode();

        // Create a forward iterator starting from our seek position.
        let iter = self
            .0
            .iterator_cf(cf, IteratorMode::From(start_bytes.as_ref(), rocksdb::Direction::Forward));

        let mut result = Vec::new();
        for item in iter {
            match item {
                Ok((key_bytes, value_bytes)) => {
                    // Decode the sharded key to check if we're still on the same address.
                    let key = ShardedKey::<Address>::decode(&key_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                    // Stop when we reach a different address (keys are sorted by address first).
                    if key.key != address {
                        break;
                    }

                    // Decompress the block number list stored in this shard.
                    let value = BlockNumberList::decompress(&value_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                    result.push((key, value));
                }
                Err(e) => {
                    return Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    })));
                }
            }
        }

        Ok(result)
    }

    /// Returns all storage history shards for the given `(address, storage_key)` pair.
    ///
    /// Iterates through all shards in ascending `highest_block_number` order until
    /// a different `(address, storage_key)` is encountered.
    pub fn storage_history_shards(
        &self,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Vec<(StorageShardedKey, BlockNumberList)>> {
        let cf = self.get_cf_handle::<tables::StoragesHistory>()?;

        let start_key = StorageShardedKey::new(address, storage_key, 0u64);
        let start_bytes = start_key.encode();

        let iter = self
            .0
            .iterator_cf(cf, IteratorMode::From(start_bytes.as_ref(), rocksdb::Direction::Forward));

        let mut result = Vec::new();
        for item in iter {
            match item {
                Ok((key_bytes, value_bytes)) => {
                    let key = StorageShardedKey::decode(&key_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                    if key.address != address || key.sharded_key.key != storage_key {
                        break;
                    }

                    let value = BlockNumberList::decompress(&value_bytes)
                        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                    result.push((key, value));
                }
                Err(e) => {
                    return Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    })));
                }
            }
        }

        Ok(result)
    }

    /// Unwinds account history indices for the given `(address, block_number)` pairs.
    ///
    /// Groups addresses by their minimum block number and calls the appropriate unwind
    /// operations. For each address, keeps only blocks less than the minimum block
    /// (i.e., removes the minimum block and all higher blocks).
    ///
    /// Returns a `WriteBatchWithTransaction` that can be committed later.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    pub fn unwind_account_history_indices(
        &self,
        last_indices: &[(Address, BlockNumber)],
    ) -> ProviderResult<WriteBatchWithTransaction<true>> {
        let mut address_min_block: AddressMap<BlockNumber> =
            AddressMap::with_capacity_and_hasher(last_indices.len(), Default::default());
        for &(address, block_number) in last_indices {
            address_min_block
                .entry(address)
                .and_modify(|min| *min = (*min).min(block_number))
                .or_insert(block_number);
        }

        let mut batch = self.batch();
        for (address, min_block) in address_min_block {
            match min_block.checked_sub(1) {
                Some(keep_to) => batch.unwind_account_history_to(address, keep_to)?,
                None => batch.clear_account_history(address)?,
            }
        }

        Ok(batch.into_inner())
    }

    /// Unwinds storage history indices for the given `(address, storage_key, block_number)` tuples.
    ///
    /// Groups by `(address, storage_key)` and finds the minimum block number for each.
    /// For each key, keeps only blocks less than the minimum block
    /// (i.e., removes the minimum block and all higher blocks).
    ///
    /// Returns a `WriteBatchWithTransaction` that can be committed later.
    pub fn unwind_storage_history_indices(
        &self,
        storage_changesets: &[(Address, B256, BlockNumber)],
    ) -> ProviderResult<WriteBatchWithTransaction<true>> {
        let mut key_min_block: HashMap<(Address, B256), BlockNumber> =
            HashMap::with_capacity_and_hasher(storage_changesets.len(), Default::default());
        for &(address, storage_key, block_number) in storage_changesets {
            key_min_block
                .entry((address, storage_key))
                .and_modify(|min| *min = (*min).min(block_number))
                .or_insert(block_number);
        }

        let mut batch = self.batch();
        for ((address, storage_key), min_block) in key_min_block {
            match min_block.checked_sub(1) {
                Some(keep_to) => batch.unwind_storage_history_to(address, storage_key, keep_to)?,
                None => batch.clear_storage_history(address, storage_key)?,
            }
        }

        Ok(batch.into_inner())
    }

    /// Writes a batch of operations atomically.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    pub fn write_batch<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut RocksDBBatch<'_>) -> ProviderResult<()>,
    {
        self.execute_with_operation_metric(RocksDBOperation::BatchWrite, "Batch", |this| {
            let mut batch_handle = this.batch();
            f(&mut batch_handle)?;
            batch_handle.commit()
        })
    }

    /// Commits a raw `WriteBatchWithTransaction` to `RocksDB`.
    ///
    /// This is used when the batch was extracted via [`RocksDBBatch::into_inner`]
    /// and needs to be committed at a later point (e.g., at provider commit time).
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all, fields(batch_len = batch.len(), batch_size = batch.size_in_bytes()))]
    pub fn commit_batch(&self, batch: WriteBatchWithTransaction<true>) -> ProviderResult<()> {
        self.0.db_rw().write_opt(batch, &WriteOptions::default()).map_err(|e| {
            ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }

    /// Writes all `RocksDB` data for multiple blocks in parallel.
    ///
    /// This handles transaction hash numbers, account history, and storage history based on
    /// the provided storage settings. Each operation runs in parallel with its own batch,
    /// pushing to `ctx.pending_batches` for later commit.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all, fields(num_blocks = blocks.len(), first_block = ctx.first_block_number))]
    pub(crate) fn write_blocks_data<N: reth_node_types::NodePrimitives>(
        &self,
        blocks: &[ExecutedBlock<N>],
        tx_nums: &[TxNumber],
        ctx: RocksDBWriteCtx,
        runtime: &reth_tasks::Runtime,
    ) -> ProviderResult<()> {
        if !ctx.storage_settings.storage_v2 {
            return Ok(());
        }

        let mut r_tx_hash = None;
        let mut r_account_history = None;
        let mut r_storage_history = None;

        let write_tx_hash =
            ctx.storage_settings.storage_v2 && ctx.prune_tx_lookup.is_none_or(|m| !m.is_full());
        let write_account_history = ctx.storage_settings.storage_v2;
        let write_storage_history = ctx.storage_settings.storage_v2;

        // Propagate tracing context into rayon-spawned threads so that RocksDB
        // write spans appear as children of write_blocks_data in traces.
        let span = tracing::Span::current();
        runtime.storage_pool().in_place_scope(|s| {
            if write_tx_hash {
                s.spawn(|_| {
                    let _guard = span.enter();
                    r_tx_hash = Some(self.write_tx_hash_numbers(blocks, tx_nums, &ctx));
                });
            }

            if write_account_history {
                s.spawn(|_| {
                    let _guard = span.enter();
                    r_account_history = Some(self.write_account_history(blocks, &ctx));
                });
            }

            if write_storage_history {
                s.spawn(|_| {
                    let _guard = span.enter();
                    r_storage_history = Some(self.write_storage_history(blocks, &ctx));
                });
            }
        });

        if write_tx_hash {
            r_tx_hash.ok_or_else(|| {
                ProviderError::Database(DatabaseError::Other(
                    "rocksdb tx-hash write thread panicked".into(),
                ))
            })??;
        }
        if write_account_history {
            r_account_history.ok_or_else(|| {
                ProviderError::Database(DatabaseError::Other(
                    "rocksdb account-history write thread panicked".into(),
                ))
            })??;
        }
        if write_storage_history {
            r_storage_history.ok_or_else(|| {
                ProviderError::Database(DatabaseError::Other(
                    "rocksdb storage-history write thread panicked".into(),
                ))
            })??;
        }

        Ok(())
    }

    /// Writes transaction hash to number mappings for the given blocks.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    fn write_tx_hash_numbers<N: reth_node_types::NodePrimitives>(
        &self,
        blocks: &[ExecutedBlock<N>],
        tx_nums: &[TxNumber],
        ctx: &RocksDBWriteCtx,
    ) -> ProviderResult<()> {
        let mut batch = self.batch();
        for (block, &first_tx_num) in blocks.iter().zip(tx_nums) {
            let body = block.recovered_block().body();
            for (tx_num, transaction) in (first_tx_num..).zip(body.transactions_iter()) {
                batch.put::<tables::TransactionHashNumbers>(*transaction.tx_hash(), &tx_num)?;
            }
        }
        ctx.pending_batches.lock().push(batch.into_inner());
        Ok(())
    }

    /// Writes account history indices for the given blocks.
    ///
    /// Derives history indices from reverts (same source as changesets) to ensure consistency.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    fn write_account_history<N: reth_node_types::NodePrimitives>(
        &self,
        blocks: &[ExecutedBlock<N>],
        ctx: &RocksDBWriteCtx,
    ) -> ProviderResult<()> {
        let mut batch = self.batch();
        let mut account_history: BTreeMap<Address, Vec<u64>> = BTreeMap::new();

        for (block_idx, block) in blocks.iter().enumerate() {
            let block_number = ctx.first_block_number + block_idx as u64;
            let reverts = block.execution_outcome().state.reverts.to_plain_state_reverts();

            // Iterate through account reverts - these are exactly the accounts that have
            // changesets written, ensuring history indices match changeset entries.
            for account_block_reverts in reverts.accounts {
                for (address, _) in account_block_reverts {
                    account_history.entry(address).or_default().push(block_number);
                }
            }
        }

        // Write account history using proper shard append logic
        for (address, indices) in account_history {
            batch.append_account_history_shard(address, indices)?;
        }
        ctx.pending_batches.lock().push(batch.into_inner());
        Ok(())
    }

    /// Writes storage history indices for the given blocks.
    ///
    /// Derives history indices from reverts (same source as changesets) to ensure consistency.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    fn write_storage_history<N: reth_node_types::NodePrimitives>(
        &self,
        blocks: &[ExecutedBlock<N>],
        ctx: &RocksDBWriteCtx,
    ) -> ProviderResult<()> {
        let mut batch = self.batch();
        let mut storage_history: BTreeMap<(Address, B256), Vec<u64>> = BTreeMap::new();

        for (block_idx, block) in blocks.iter().enumerate() {
            let block_number = ctx.first_block_number + block_idx as u64;
            let reverts = block.execution_outcome().state.reverts.to_plain_state_reverts();

            // Iterate through storage reverts - these are exactly the slots that have
            // changesets written, ensuring history indices match changeset entries.
            for storage_block_reverts in reverts.storage {
                for revert in storage_block_reverts {
                    for (slot, _) in revert.storage_revert {
                        let plain_key = B256::new(slot.to_be_bytes());
                        storage_history
                            .entry((revert.address, plain_key))
                            .or_default()
                            .push(block_number);
                    }
                }
            }
        }

        // Write storage history using proper shard append logic
        for ((address, slot), indices) in storage_history {
            batch.append_storage_history_shard(address, slot, indices)?;
        }
        ctx.pending_batches.lock().push(batch.into_inner());
        Ok(())
    }
}

/// Outcome of pruning a history shard in `RocksDB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneShardOutcome {
    /// Shard was deleted entirely.
    Deleted,
    /// Shard was updated with filtered block numbers.
    Updated,
    /// Shard was unchanged (no blocks <= `to_block`).
    Unchanged,
}

/// Tracks pruning outcomes for batch operations.
#[derive(Debug, Default, Clone, Copy)]
pub struct PrunedIndices {
    /// Number of shards completely deleted.
    pub deleted: usize,
    /// Number of shards that were updated (filtered but still have entries).
    pub updated: usize,
    /// Number of shards that were unchanged.
    pub unchanged: usize,
}

/// Handle for building a batch of operations atomically.
///
/// Uses `WriteBatchWithTransaction` for atomic writes without full transaction overhead.
/// Unlike [`RocksTx`], this does NOT support read-your-writes. Use for write-only flows
/// where you don't need to read back uncommitted data within the same operation
/// (e.g., history index writes).
///
/// When `auto_commit_threshold` is set, the batch will automatically commit and reset
/// when the batch size exceeds the threshold. This prevents OOM during large bulk writes.
#[must_use = "batch must be committed"]
pub struct RocksDBBatch<'a> {
    provider: &'a RocksDBProvider,
    inner: WriteBatchWithTransaction<true>,
    buf: Vec<u8>,
    /// If set, batch auto-commits when size exceeds this threshold (in bytes).
    auto_commit_threshold: Option<usize>,
}

impl fmt::Debug for RocksDBBatch<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBBatch")
            .field("provider", &self.provider)
            .field("batch", &"<WriteBatchWithTransaction>")
            // Number of operations in this batch
            .field("length", &self.inner.len())
            // Total serialized size (encoded key + compressed value + metadata) of this batch
            // in bytes
            .field("size_in_bytes", &self.inner.size_in_bytes())
            .finish()
    }
}

impl<'a> RocksDBBatch<'a> {
    /// Puts a value into the batch.
    ///
    /// If auto-commit is enabled and the batch exceeds the threshold, commits and resets.
    pub fn put<T: Table>(&mut self, key: T::Key, value: &T::Value) -> ProviderResult<()> {
        let encoded_key = key.encode();
        self.put_encoded::<T>(&encoded_key, value)
    }

    /// Puts a value into the batch using pre-encoded key.
    ///
    /// If auto-commit is enabled and the batch exceeds the threshold, commits and resets.
    pub fn put_encoded<T: Table>(
        &mut self,
        key: &<T::Key as Encode>::Encoded,
        value: &T::Value,
    ) -> ProviderResult<()> {
        let value_bytes = compress_to_buf_or_ref!(self.buf, value).unwrap_or(&self.buf);
        self.inner.put_cf(self.provider.get_cf_handle::<T>()?, key, value_bytes);
        self.maybe_auto_commit()?;
        Ok(())
    }

    /// Deletes a value from the batch.
    ///
    /// If auto-commit is enabled and the batch exceeds the threshold, commits and resets.
    pub fn delete<T: Table>(&mut self, key: T::Key) -> ProviderResult<()> {
        self.inner.delete_cf(self.provider.get_cf_handle::<T>()?, key.encode().as_ref());
        self.maybe_auto_commit()?;
        Ok(())
    }

    /// Commits and resets the batch if it exceeds the auto-commit threshold.
    ///
    /// This is called after each `put` or `delete` operation to prevent unbounded memory growth.
    /// Returns immediately if auto-commit is disabled or threshold not reached.
    fn maybe_auto_commit(&mut self) -> ProviderResult<()> {
        if let Some(threshold) = self.auto_commit_threshold &&
            self.inner.size_in_bytes() >= threshold
        {
            tracing::debug!(
                target: "providers::rocksdb",
                batch_size = self.inner.size_in_bytes(),
                threshold,
                "Auto-committing RocksDB batch"
            );
            let old_batch = std::mem::take(&mut self.inner);
            self.provider.0.db_rw().write_opt(old_batch, &WriteOptions::default()).map_err(
                |e| {
                    ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    }))
                },
            )?;
        }
        Ok(())
    }

    /// Commits the batch to the database.
    ///
    /// This consumes the batch and writes all operations atomically to `RocksDB`.
    ///
    /// # Panics
    /// Panics if the provider is in read-only mode.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all, fields(batch_len = self.inner.len(), batch_size = self.inner.size_in_bytes()))]
    pub fn commit(self) -> ProviderResult<()> {
        self.provider.0.db_rw().write_opt(self.inner, &WriteOptions::default()).map_err(|e| {
            ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }

    /// Returns the number of write operations (puts + deletes) queued in this batch.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the batch contains no operations.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the size of the batch in bytes.
    pub fn size_in_bytes(&self) -> usize {
        self.inner.size_in_bytes()
    }

    /// Returns a reference to the underlying `RocksDB` provider.
    pub const fn provider(&self) -> &RocksDBProvider {
        self.provider
    }

    /// Consumes the batch and returns the underlying `WriteBatchWithTransaction`.
    ///
    /// This is used to defer commits to the provider level.
    pub fn into_inner(self) -> WriteBatchWithTransaction<true> {
        self.inner
    }

    /// Gets a value from the database.
    ///
    /// **Important constraint:** This reads only committed state, not pending writes in this
    /// batch or other pending batches in `pending_rocksdb_batches`.
    pub fn get<T: Table>(&self, key: T::Key) -> ProviderResult<Option<T::Value>> {
        self.provider.get::<T>(key)
    }

    /// Appends indices to an account history shard with proper shard management.
    ///
    /// Loads the existing shard (if any), appends new indices, and rechunks into
    /// multiple shards if needed (respecting `NUM_OF_INDICES_IN_SHARD` limit).
    ///
    /// # Requirements
    ///
    /// - The `indices` MUST be strictly increasing and contain no duplicates.
    /// - This method MUST only be called once per address per batch. The batch reads existing
    ///   shards from committed DB state, not from pending writes. Calling twice for the same
    ///   address will cause the second call to overwrite the first.
    pub fn append_account_history_shard(
        &mut self,
        address: Address,
        indices: impl IntoIterator<Item = u64>,
    ) -> ProviderResult<()> {
        let indices: Vec<u64> = indices.into_iter().collect();

        if indices.is_empty() {
            return Ok(());
        }

        debug_assert!(
            indices.windows(2).all(|w| w[0] < w[1]),
            "indices must be strictly increasing: {:?}",
            indices
        );

        let last_key = ShardedKey::new(address, u64::MAX);
        let last_shard_opt = self.provider.get::<tables::AccountsHistory>(last_key.clone())?;
        let mut last_shard = last_shard_opt.unwrap_or_else(BlockNumberList::empty);

        last_shard.append(indices).map_err(ProviderError::other)?;

        // Fast path: all indices fit in one shard
        if last_shard.len() <= NUM_OF_INDICES_IN_SHARD as u64 {
            self.put::<tables::AccountsHistory>(last_key, &last_shard)?;
            return Ok(());
        }

        // Slow path: rechunk into multiple shards
        let chunks = last_shard.iter().chunks(NUM_OF_INDICES_IN_SHARD);
        let mut chunks_peekable = chunks.into_iter().peekable();

        while let Some(chunk) = chunks_peekable.next() {
            let shard = BlockNumberList::new_pre_sorted(chunk);
            let highest_block_number = if chunks_peekable.peek().is_some() {
                shard.iter().next_back().expect("`chunks` does not return empty list")
            } else {
                u64::MAX
            };

            self.put::<tables::AccountsHistory>(
                ShardedKey::new(address, highest_block_number),
                &shard,
            )?;
        }

        Ok(())
    }

    /// Appends indices to a storage history shard with proper shard management.
    ///
    /// Loads the existing shard (if any), appends new indices, and rechunks into
    /// multiple shards if needed (respecting `NUM_OF_INDICES_IN_SHARD` limit).
    ///
    /// # Requirements
    ///
    /// - The `indices` MUST be strictly increasing and contain no duplicates.
    /// - This method MUST only be called once per (address, `storage_key`) pair per batch. The
    ///   batch reads existing shards from committed DB state, not from pending writes. Calling
    ///   twice for the same key will cause the second call to overwrite the first.
    pub fn append_storage_history_shard(
        &mut self,
        address: Address,
        storage_key: B256,
        indices: impl IntoIterator<Item = u64>,
    ) -> ProviderResult<()> {
        let indices: Vec<u64> = indices.into_iter().collect();

        if indices.is_empty() {
            return Ok(());
        }

        debug_assert!(
            indices.windows(2).all(|w| w[0] < w[1]),
            "indices must be strictly increasing: {:?}",
            indices
        );

        let last_key = StorageShardedKey::last(address, storage_key);
        let last_shard_opt = self.provider.get::<tables::StoragesHistory>(last_key.clone())?;
        let mut last_shard = last_shard_opt.unwrap_or_else(BlockNumberList::empty);

        last_shard.append(indices).map_err(ProviderError::other)?;

        // Fast path: all indices fit in one shard
        if last_shard.len() <= NUM_OF_INDICES_IN_SHARD as u64 {
            self.put::<tables::StoragesHistory>(last_key, &last_shard)?;
            return Ok(());
        }

        // Slow path: rechunk into multiple shards
        let chunks = last_shard.iter().chunks(NUM_OF_INDICES_IN_SHARD);
        let mut chunks_peekable = chunks.into_iter().peekable();

        while let Some(chunk) = chunks_peekable.next() {
            let shard = BlockNumberList::new_pre_sorted(chunk);
            let highest_block_number = if chunks_peekable.peek().is_some() {
                shard.iter().next_back().expect("`chunks` does not return empty list")
            } else {
                u64::MAX
            };

            self.put::<tables::StoragesHistory>(
                StorageShardedKey::new(address, storage_key, highest_block_number),
                &shard,
            )?;
        }

        Ok(())
    }

    /// Unwinds account history for the given address, keeping only blocks <= `keep_to`.
    ///
    /// Mirrors MDBX `unwind_history_shards` behavior:
    /// - Deletes shards entirely above `keep_to`
    /// - Truncates boundary shards and re-keys to `u64::MAX` sentinel
    /// - Preserves shards entirely below `keep_to`
    pub fn unwind_account_history_to(
        &mut self,
        address: Address,
        keep_to: BlockNumber,
    ) -> ProviderResult<()> {
        let shards = self.provider.account_history_shards(address)?;
        if shards.is_empty() {
            return Ok(());
        }

        // Find the first shard that might contain blocks > keep_to.
        // A shard is affected if it's the sentinel (u64::MAX) or its highest_block_number > keep_to
        let boundary_idx = shards.iter().position(|(key, _)| {
            key.highest_block_number == u64::MAX || key.highest_block_number > keep_to
        });

        // Repair path: no shards affected means all blocks <= keep_to, just ensure sentinel exists
        let Some(boundary_idx) = boundary_idx else {
            let (last_key, last_value) = shards.last().expect("shards is non-empty");
            if last_key.highest_block_number != u64::MAX {
                self.delete::<tables::AccountsHistory>(last_key.clone())?;
                self.put::<tables::AccountsHistory>(
                    ShardedKey::new(address, u64::MAX),
                    last_value,
                )?;
            }
            return Ok(());
        };

        // Delete all shards strictly after the boundary (they are entirely > keep_to)
        for (key, _) in shards.iter().skip(boundary_idx + 1) {
            self.delete::<tables::AccountsHistory>(key.clone())?;
        }

        // Process the boundary shard: filter out blocks > keep_to
        let (boundary_key, boundary_list) = &shards[boundary_idx];

        // Delete the boundary shard (we'll either drop it or rewrite at u64::MAX)
        self.delete::<tables::AccountsHistory>(boundary_key.clone())?;

        // Build truncated list once; check emptiness directly (avoids double iteration)
        let new_last =
            BlockNumberList::new_pre_sorted(boundary_list.iter().take_while(|&b| b <= keep_to));

        if new_last.is_empty() {
            // Boundary shard is now empty. Previous shard becomes the last and must be keyed
            // u64::MAX.
            if boundary_idx == 0 {
                // Nothing left for this address
                return Ok(());
            }

            let (prev_key, prev_value) = &shards[boundary_idx - 1];
            if prev_key.highest_block_number != u64::MAX {
                self.delete::<tables::AccountsHistory>(prev_key.clone())?;
                self.put::<tables::AccountsHistory>(
                    ShardedKey::new(address, u64::MAX),
                    prev_value,
                )?;
            }
            return Ok(());
        }

        self.put::<tables::AccountsHistory>(ShardedKey::new(address, u64::MAX), &new_last)?;

        Ok(())
    }

    /// Prunes history shards, removing blocks <= `to_block`.
    ///
    /// Generic implementation for both account and storage history pruning.
    /// Mirrors MDBX `prune_shard` semantics. After pruning, the last remaining shard
    /// (if any) will have the sentinel key (`u64::MAX`).
    #[allow(clippy::too_many_arguments)]
    fn prune_history_shards_inner<K>(
        &mut self,
        shards: Vec<(K, BlockNumberList)>,
        to_block: BlockNumber,
        get_highest: impl Fn(&K) -> u64,
        is_sentinel: impl Fn(&K) -> bool,
        delete_shard: impl Fn(&mut Self, K) -> ProviderResult<()>,
        put_shard: impl Fn(&mut Self, K, &BlockNumberList) -> ProviderResult<()>,
        create_sentinel: impl Fn() -> K,
    ) -> ProviderResult<PruneShardOutcome>
    where
        K: Clone,
    {
        if shards.is_empty() {
            return Ok(PruneShardOutcome::Unchanged);
        }

        let mut deleted = false;
        let mut updated = false;
        let mut last_remaining: Option<(K, BlockNumberList)> = None;

        for (key, block_list) in shards {
            if !is_sentinel(&key) && get_highest(&key) <= to_block {
                delete_shard(self, key)?;
                deleted = true;
            } else {
                let original_len = block_list.len();
                let filtered =
                    BlockNumberList::new_pre_sorted(block_list.iter().filter(|&b| b > to_block));

                if filtered.is_empty() {
                    delete_shard(self, key)?;
                    deleted = true;
                } else if filtered.len() < original_len {
                    put_shard(self, key.clone(), &filtered)?;
                    last_remaining = Some((key, filtered));
                    updated = true;
                } else {
                    last_remaining = Some((key, block_list));
                }
            }
        }

        if let Some((last_key, last_value)) = last_remaining &&
            !is_sentinel(&last_key)
        {
            delete_shard(self, last_key)?;
            put_shard(self, create_sentinel(), &last_value)?;
            updated = true;
        }

        if deleted {
            Ok(PruneShardOutcome::Deleted)
        } else if updated {
            Ok(PruneShardOutcome::Updated)
        } else {
            Ok(PruneShardOutcome::Unchanged)
        }
    }

    /// Prunes account history for the given address, removing blocks <= `to_block`.
    ///
    /// Mirrors MDBX `prune_shard` semantics. After pruning, the last remaining shard
    /// (if any) will have the sentinel key (`u64::MAX`).
    pub fn prune_account_history_to(
        &mut self,
        address: Address,
        to_block: BlockNumber,
    ) -> ProviderResult<PruneShardOutcome> {
        let shards = self.provider.account_history_shards(address)?;
        self.prune_history_shards_inner(
            shards,
            to_block,
            |key| key.highest_block_number,
            |key| key.highest_block_number == u64::MAX,
            |batch, key| batch.delete::<tables::AccountsHistory>(key),
            |batch, key, value| batch.put::<tables::AccountsHistory>(key, value),
            || ShardedKey::new(address, u64::MAX),
        )
    }

    /// Prunes account history for multiple addresses in a single iterator pass.
    ///
    /// This is more efficient than calling [`Self::prune_account_history_to`] repeatedly
    /// because it reuses a single raw iterator and skips seeks when the iterator is already
    /// positioned correctly (which happens when targets are sorted and adjacent in key order).
    ///
    /// `targets` MUST be sorted by address for correctness and optimal performance
    /// (matches on-disk key order).
    pub fn prune_account_history_batch(
        &mut self,
        targets: &[(Address, BlockNumber)],
    ) -> ProviderResult<PrunedIndices> {
        if targets.is_empty() {
            return Ok(PrunedIndices::default());
        }

        debug_assert!(
            targets.windows(2).all(|w| w[0].0 <= w[1].0),
            "prune_account_history_batch: targets must be sorted by address"
        );

        // ShardedKey<Address> layout: [address: 20][block: 8] = 28 bytes
        // The first 20 bytes are the "prefix" that identifies the address
        const PREFIX_LEN: usize = 20;

        let cf = self.provider.get_cf_handle::<tables::AccountsHistory>()?;
        let mut iter = self.provider.0.raw_iterator_cf(cf);
        let mut outcomes = PrunedIndices::default();

        for (address, to_block) in targets {
            // Build the target prefix (first 20 bytes = address)
            let start_key = ShardedKey::new(*address, 0u64).encode();
            let target_prefix = &start_key[..PREFIX_LEN];

            // Check if we need to seek or if the iterator is already positioned correctly.
            // After processing the previous target, the iterator is either:
            // 1. Positioned at a key with a different prefix (we iterated past our shards)
            // 2. Invalid (no more keys)
            // If the current key's prefix >= our target prefix, we may be able to skip the seek.
            let needs_seek = if iter.valid() {
                if let Some(current_key) = iter.key() {
                    // If current key's prefix < target prefix, we need to seek forward
                    // If current key's prefix > target prefix, this target has no shards (skip)
                    // If current key's prefix == target prefix, we're already positioned
                    current_key.get(..PREFIX_LEN).is_none_or(|p| p < target_prefix)
                } else {
                    true
                }
            } else {
                true
            };

            if needs_seek {
                iter.seek(start_key);
                iter.status().map_err(|e| {
                    ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    }))
                })?;
            }

            // Collect all shards for this address using raw prefix comparison
            let mut shards = Vec::new();
            while iter.valid() {
                let Some(key_bytes) = iter.key() else { break };

                // Use raw prefix comparison instead of full decode for the prefix check
                let current_prefix = key_bytes.get(..PREFIX_LEN);
                if current_prefix != Some(target_prefix) {
                    break;
                }

                // Now decode the full key (we need the block number)
                let key = ShardedKey::<Address>::decode(key_bytes)
                    .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                let Some(value_bytes) = iter.value() else { break };
                let value = BlockNumberList::decompress(value_bytes)
                    .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                shards.push((key, value));
                iter.next();
            }

            match self.prune_history_shards_inner(
                shards,
                *to_block,
                |key| key.highest_block_number,
                |key| key.highest_block_number == u64::MAX,
                |batch, key| batch.delete::<tables::AccountsHistory>(key),
                |batch, key, value| batch.put::<tables::AccountsHistory>(key, value),
                || ShardedKey::new(*address, u64::MAX),
            )? {
                PruneShardOutcome::Deleted => outcomes.deleted += 1,
                PruneShardOutcome::Updated => outcomes.updated += 1,
                PruneShardOutcome::Unchanged => outcomes.unchanged += 1,
            }
        }

        Ok(outcomes)
    }

    /// Prunes storage history for the given address and storage key, removing blocks <=
    /// `to_block`.
    ///
    /// Mirrors MDBX `prune_shard` semantics. After pruning, the last remaining shard
    /// (if any) will have the sentinel key (`u64::MAX`).
    pub fn prune_storage_history_to(
        &mut self,
        address: Address,
        storage_key: B256,
        to_block: BlockNumber,
    ) -> ProviderResult<PruneShardOutcome> {
        let shards = self.provider.storage_history_shards(address, storage_key)?;
        self.prune_history_shards_inner(
            shards,
            to_block,
            |key| key.sharded_key.highest_block_number,
            |key| key.sharded_key.highest_block_number == u64::MAX,
            |batch, key| batch.delete::<tables::StoragesHistory>(key),
            |batch, key, value| batch.put::<tables::StoragesHistory>(key, value),
            || StorageShardedKey::last(address, storage_key),
        )
    }

    /// Prunes storage history for multiple (address, `storage_key`) pairs in a single iterator
    /// pass.
    ///
    /// This is more efficient than calling [`Self::prune_storage_history_to`] repeatedly
    /// because it reuses a single raw iterator and skips seeks when the iterator is already
    /// positioned correctly (which happens when targets are sorted and adjacent in key order).
    ///
    /// `targets` MUST be sorted by (address, `storage_key`) for correctness and optimal
    /// performance (matches on-disk key order).
    pub fn prune_storage_history_batch(
        &mut self,
        targets: &[((Address, B256), BlockNumber)],
    ) -> ProviderResult<PrunedIndices> {
        if targets.is_empty() {
            return Ok(PrunedIndices::default());
        }

        debug_assert!(
            targets.windows(2).all(|w| w[0].0 <= w[1].0),
            "prune_storage_history_batch: targets must be sorted by (address, storage_key)"
        );

        // StorageShardedKey layout: [address: 20][storage_key: 32][block: 8] = 60 bytes
        // The first 52 bytes are the "prefix" that identifies (address, storage_key)
        const PREFIX_LEN: usize = 52;

        let cf = self.provider.get_cf_handle::<tables::StoragesHistory>()?;
        let mut iter = self.provider.0.raw_iterator_cf(cf);
        let mut outcomes = PrunedIndices::default();

        for ((address, storage_key), to_block) in targets {
            // Build the target prefix (first 52 bytes of encoded key)
            let start_key = StorageShardedKey::new(*address, *storage_key, 0u64).encode();
            let target_prefix = &start_key[..PREFIX_LEN];

            // Check if we need to seek or if the iterator is already positioned correctly.
            // After processing the previous target, the iterator is either:
            // 1. Positioned at a key with a different prefix (we iterated past our shards)
            // 2. Invalid (no more keys)
            // If the current key's prefix >= our target prefix, we may be able to skip the seek.
            let needs_seek = if iter.valid() {
                if let Some(current_key) = iter.key() {
                    // If current key's prefix < target prefix, we need to seek forward
                    // If current key's prefix > target prefix, this target has no shards (skip)
                    // If current key's prefix == target prefix, we're already positioned
                    current_key.get(..PREFIX_LEN).is_none_or(|p| p < target_prefix)
                } else {
                    true
                }
            } else {
                true
            };

            if needs_seek {
                iter.seek(start_key);
                iter.status().map_err(|e| {
                    ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    }))
                })?;
            }

            // Collect all shards for this (address, storage_key) pair using prefix comparison
            let mut shards = Vec::new();
            while iter.valid() {
                let Some(key_bytes) = iter.key() else { break };

                // Use raw prefix comparison instead of full decode for the prefix check
                let current_prefix = key_bytes.get(..PREFIX_LEN);
                if current_prefix != Some(target_prefix) {
                    break;
                }

                // Now decode the full key (we need the block number)
                let key = StorageShardedKey::decode(key_bytes)
                    .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                let Some(value_bytes) = iter.value() else { break };
                let value = BlockNumberList::decompress(value_bytes)
                    .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

                shards.push((key, value));
                iter.next();
            }

            // Use existing prune_history_shards_inner logic
            match self.prune_history_shards_inner(
                shards,
                *to_block,
                |key| key.sharded_key.highest_block_number,
                |key| key.sharded_key.highest_block_number == u64::MAX,
                |batch, key| batch.delete::<tables::StoragesHistory>(key),
                |batch, key, value| batch.put::<tables::StoragesHistory>(key, value),
                || StorageShardedKey::last(*address, *storage_key),
            )? {
                PruneShardOutcome::Deleted => outcomes.deleted += 1,
                PruneShardOutcome::Updated => outcomes.updated += 1,
                PruneShardOutcome::Unchanged => outcomes.unchanged += 1,
            }
        }

        Ok(outcomes)
    }

    /// Unwinds storage history to keep only blocks `<= keep_to`.
    ///
    /// Handles multi-shard scenarios by:
    /// 1. Loading all shards for the `(address, storage_key)` pair
    /// 2. Finding the boundary shard containing `keep_to`
    /// 3. Deleting all shards after the boundary
    /// 4. Truncating the boundary shard to keep only indices `<= keep_to`
    /// 5. Ensuring the last shard is keyed with `u64::MAX`
    pub fn unwind_storage_history_to(
        &mut self,
        address: Address,
        storage_key: B256,
        keep_to: BlockNumber,
    ) -> ProviderResult<()> {
        let shards = self.provider.storage_history_shards(address, storage_key)?;
        if shards.is_empty() {
            return Ok(());
        }

        // Find the first shard that might contain blocks > keep_to.
        // A shard is affected if it's the sentinel (u64::MAX) or its highest_block_number > keep_to
        let boundary_idx = shards.iter().position(|(key, _)| {
            key.sharded_key.highest_block_number == u64::MAX ||
                key.sharded_key.highest_block_number > keep_to
        });

        // Repair path: no shards affected means all blocks <= keep_to, just ensure sentinel exists
        let Some(boundary_idx) = boundary_idx else {
            let (last_key, last_value) = shards.last().expect("shards is non-empty");
            if last_key.sharded_key.highest_block_number != u64::MAX {
                self.delete::<tables::StoragesHistory>(last_key.clone())?;
                self.put::<tables::StoragesHistory>(
                    StorageShardedKey::last(address, storage_key),
                    last_value,
                )?;
            }
            return Ok(());
        };

        // Delete all shards strictly after the boundary (they are entirely > keep_to)
        for (key, _) in shards.iter().skip(boundary_idx + 1) {
            self.delete::<tables::StoragesHistory>(key.clone())?;
        }

        // Process the boundary shard: filter out blocks > keep_to
        let (boundary_key, boundary_list) = &shards[boundary_idx];

        // Delete the boundary shard (we'll either drop it or rewrite at u64::MAX)
        self.delete::<tables::StoragesHistory>(boundary_key.clone())?;

        // Build truncated list once; check emptiness directly (avoids double iteration)
        let new_last =
            BlockNumberList::new_pre_sorted(boundary_list.iter().take_while(|&b| b <= keep_to));

        if new_last.is_empty() {
            // Boundary shard is now empty. Previous shard becomes the last and must be keyed
            // u64::MAX.
            if boundary_idx == 0 {
                // Nothing left for this (address, storage_key) pair
                return Ok(());
            }

            let (prev_key, prev_value) = &shards[boundary_idx - 1];
            if prev_key.sharded_key.highest_block_number != u64::MAX {
                self.delete::<tables::StoragesHistory>(prev_key.clone())?;
                self.put::<tables::StoragesHistory>(
                    StorageShardedKey::last(address, storage_key),
                    prev_value,
                )?;
            }
            return Ok(());
        }

        self.put::<tables::StoragesHistory>(
            StorageShardedKey::last(address, storage_key),
            &new_last,
        )?;

        Ok(())
    }

    /// Clears all account history shards for the given address.
    ///
    /// Used when unwinding from block 0 (i.e., removing all history).
    pub fn clear_account_history(&mut self, address: Address) -> ProviderResult<()> {
        let shards = self.provider.account_history_shards(address)?;
        for (key, _) in shards {
            self.delete::<tables::AccountsHistory>(key)?;
        }
        Ok(())
    }

    /// Clears all storage history shards for the given `(address, storage_key)` pair.
    ///
    /// Used when unwinding from block 0 (i.e., removing all history for this storage slot).
    pub fn clear_storage_history(
        &mut self,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<()> {
        let shards = self.provider.storage_history_shards(address, storage_key)?;
        for (key, _) in shards {
            self.delete::<tables::StoragesHistory>(key)?;
        }
        Ok(())
    }
}

/// `RocksDB` transaction wrapper providing MDBX-like semantics.
///
/// Supports:
/// - Read-your-writes: reads see uncommitted writes within the same transaction
/// - Atomic commit/rollback
/// - Iteration over uncommitted data
///
/// Note: `Transaction` is `Send` but NOT `Sync`. This wrapper does not implement
/// `DbTx`/`DbTxMut` traits directly; use RocksDB-specific methods instead.
pub struct RocksTx<'db> {
    inner: Transaction<'db, OptimisticTransactionDB>,
    provider: &'db RocksDBProvider,
}

impl fmt::Debug for RocksTx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksTx").field("provider", &self.provider).finish_non_exhaustive()
    }
}

impl<'db> RocksTx<'db> {
    /// Gets a value from the specified table. Sees uncommitted writes in this transaction.
    pub fn get<T: Table>(&self, key: T::Key) -> ProviderResult<Option<T::Value>> {
        let encoded_key = key.encode();
        self.get_encoded::<T>(&encoded_key)
    }

    /// Gets a value using pre-encoded key. Sees uncommitted writes in this transaction.
    pub fn get_encoded<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
    ) -> ProviderResult<Option<T::Value>> {
        let cf = self.provider.get_cf_handle::<T>()?;
        let result = self.inner.get_cf(cf, key.as_ref()).map_err(|e| {
            ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })?;

        Ok(result.and_then(|value| T::Value::decompress(&value).ok()))
    }

    /// Puts a value into the specified table.
    pub fn put<T: Table>(&self, key: T::Key, value: &T::Value) -> ProviderResult<()> {
        let encoded_key = key.encode();
        self.put_encoded::<T>(&encoded_key, value)
    }

    /// Puts a value using pre-encoded key.
    pub fn put_encoded<T: Table>(
        &self,
        key: &<T::Key as Encode>::Encoded,
        value: &T::Value,
    ) -> ProviderResult<()> {
        let cf = self.provider.get_cf_handle::<T>()?;
        let mut buf = Vec::new();
        let value_bytes = compress_to_buf_or_ref!(buf, value).unwrap_or(&buf);

        self.inner.put_cf(cf, key.as_ref(), value_bytes).map_err(|e| {
            ProviderError::Database(DatabaseError::Write(Box::new(DatabaseWriteError {
                info: DatabaseErrorInfo { message: e.to_string().into(), code: -1 },
                operation: DatabaseWriteOperation::PutUpsert,
                table_name: T::NAME,
                key: key.as_ref().to_vec(),
            })))
        })
    }

    /// Deletes a value from the specified table.
    pub fn delete<T: Table>(&self, key: T::Key) -> ProviderResult<()> {
        let cf = self.provider.get_cf_handle::<T>()?;
        self.inner.delete_cf(cf, key.encode().as_ref()).map_err(|e| {
            ProviderError::Database(DatabaseError::Delete(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }

    /// Creates an iterator for the specified table. Sees uncommitted writes in this transaction.
    ///
    /// Returns an iterator that yields `(encoded_key, compressed_value)` pairs.
    pub fn iter<T: Table>(&self) -> ProviderResult<RocksTxIter<'_, T>> {
        let cf = self.provider.get_cf_handle::<T>()?;
        let iter = self.inner.iterator_cf(cf, IteratorMode::Start);
        Ok(RocksTxIter { inner: iter, _marker: std::marker::PhantomData })
    }

    /// Creates an iterator starting from the given key (inclusive).
    pub fn iter_from<T: Table>(&self, key: T::Key) -> ProviderResult<RocksTxIter<'_, T>> {
        let cf = self.provider.get_cf_handle::<T>()?;
        let encoded_key = key.encode();
        let iter = self
            .inner
            .iterator_cf(cf, IteratorMode::From(encoded_key.as_ref(), rocksdb::Direction::Forward));
        Ok(RocksTxIter { inner: iter, _marker: std::marker::PhantomData })
    }

    /// Commits the transaction, persisting all changes.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    pub fn commit(self) -> ProviderResult<()> {
        self.inner.commit().map_err(|e| {
            ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }

    /// Rolls back the transaction, discarding all changes.
    #[instrument(level = "debug", target = "providers::rocksdb", skip_all)]
    pub fn rollback(self) -> ProviderResult<()> {
        self.inner.rollback().map_err(|e| {
            ProviderError::Database(DatabaseError::Other(format!("rollback failed: {e}")))
        })
    }

    /// Lookup account history and return [`HistoryInfo`] directly.
    ///
    /// This is a thin wrapper around `history_info` that:
    /// - Builds the `ShardedKey` for the address + target block.
    /// - Validates that the found shard belongs to the same address.
    pub fn account_history_info(
        &self,
        address: Address,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        let key = ShardedKey::new(address, block_number);
        self.history_info::<tables::AccountsHistory>(
            key.encode().as_ref(),
            block_number,
            lowest_available_block_number,
            |key_bytes| Ok(<ShardedKey<Address> as Decode>::decode(key_bytes)?.key == address),
            |prev_bytes| {
                <ShardedKey<Address> as Decode>::decode(prev_bytes)
                    .map(|k| k.key == address)
                    .unwrap_or(false)
            },
        )
    }

    /// Lookup storage history and return [`HistoryInfo`] directly.
    ///
    /// This is a thin wrapper around `history_info` that:
    /// - Builds the `StorageShardedKey` for address + storage key + target block.
    /// - Validates that the found shard belongs to the same address and storage slot.
    pub fn storage_history_info(
        &self,
        address: Address,
        storage_key: B256,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        let key = StorageShardedKey::new(address, storage_key, block_number);
        self.history_info::<tables::StoragesHistory>(
            key.encode().as_ref(),
            block_number,
            lowest_available_block_number,
            |key_bytes| {
                let k = <StorageShardedKey as Decode>::decode(key_bytes)?;
                Ok(k.address == address && k.sharded_key.key == storage_key)
            },
            |prev_bytes| {
                <StorageShardedKey as Decode>::decode(prev_bytes)
                    .map(|k| k.address == address && k.sharded_key.key == storage_key)
                    .unwrap_or(false)
            },
        )
    }

    /// Generic history lookup for sharded history tables.
    ///
    /// Seeks to the shard containing `block_number`, checks if the key matches via `key_matches`,
    /// and uses `prev_key_matches` to detect if a previous shard exists for the same key.
    fn history_info<T>(
        &self,
        encoded_key: &[u8],
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
        key_matches: impl FnOnce(&[u8]) -> Result<bool, reth_db_api::DatabaseError>,
        prev_key_matches: impl Fn(&[u8]) -> bool,
    ) -> ProviderResult<HistoryInfo>
    where
        T: Table<Value = BlockNumberList>,
    {
        // History may be pruned if a lowest available block is set.
        let is_maybe_pruned = lowest_available_block_number.is_some();
        let fallback = || {
            Ok(if is_maybe_pruned {
                HistoryInfo::MaybeInPlainState
            } else {
                HistoryInfo::NotYetWritten
            })
        };

        let cf = self.provider.0.cf_handle_rw(T::NAME)?;

        // Create a raw iterator to access key bytes directly.
        let mut iter: DBRawIteratorWithThreadMode<'_, Transaction<'_, OptimisticTransactionDB>> =
            self.inner.raw_iterator_cf(&cf);

        // Seek to the smallest key >= encoded_key.
        iter.seek(encoded_key);
        Self::raw_iter_status_ok(&iter)?;

        if !iter.valid() {
            // No shard found at or after target block.
            //
            // (MaybeInPlainState) The key may have been written, but due to pruning we may not have
            // changesets and history, so we need to make a plain state lookup.
            // (HistoryInfo::NotYetWritten) The key has not been written to at all.
            return fallback();
        }

        // Check if the found key matches our target entity.
        let Some(key_bytes) = iter.key() else {
            return fallback();
        };
        if !key_matches(key_bytes)? {
            // The found key is for a different entity.
            return fallback();
        }

        // Decompress the block list for this shard.
        let Some(value_bytes) = iter.value() else {
            return fallback();
        };
        let chunk = BlockNumberList::decompress(value_bytes)?;
        let (rank, found_block) = compute_history_rank(&chunk, block_number);

        // Lazy check for previous shard - only called when needed.
        // If we can step to a previous shard for this same key, history already exists,
        // so the target block is not before the first write.
        let is_before_first_write = if needs_prev_shard_check(rank, found_block, block_number) {
            iter.prev();
            Self::raw_iter_status_ok(&iter)?;
            let has_prev = iter.valid() && iter.key().is_some_and(&prev_key_matches);
            !has_prev
        } else {
            false
        };

        Ok(HistoryInfo::from_lookup(
            found_block,
            is_before_first_write,
            lowest_available_block_number,
        ))
    }

    /// Returns an error if the raw iterator is in an invalid state due to an I/O error.
    fn raw_iter_status_ok(
        iter: &DBRawIteratorWithThreadMode<'_, Transaction<'_, OptimisticTransactionDB>>,
    ) -> ProviderResult<()> {
        iter.status().map_err(|e| {
            ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }
}

/// Wrapper enum for `RocksDB` iterators that works in both read-write and read-only modes.
enum RocksDBIterEnum<'db> {
    /// Iterator from read-write `OptimisticTransactionDB`.
    ReadWrite(rocksdb::DBIteratorWithThreadMode<'db, OptimisticTransactionDB>),
    /// Iterator from read-only `DB`.
    ReadOnly(rocksdb::DBIteratorWithThreadMode<'db, DB>),
}

impl Iterator for RocksDBIterEnum<'_> {
    type Item = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::ReadWrite(iter) => iter.next(),
            Self::ReadOnly(iter) => iter.next(),
        }
    }
}

/// Wrapper enum for raw `RocksDB` iterators that works in both read-write and read-only modes.
///
/// Unlike [`RocksDBIterEnum`], raw iterators expose `seek()` for efficient repositioning
/// without reinitializing the iterator.
enum RocksDBRawIterEnum<'db> {
    /// Raw iterator from read-write `OptimisticTransactionDB`.
    ReadWrite(DBRawIteratorWithThreadMode<'db, OptimisticTransactionDB>),
    /// Raw iterator from read-only `DB`.
    ReadOnly(DBRawIteratorWithThreadMode<'db, DB>),
}

impl RocksDBRawIterEnum<'_> {
    /// Positions the iterator at the first key >= `key`.
    fn seek(&mut self, key: impl AsRef<[u8]>) {
        match self {
            Self::ReadWrite(iter) => iter.seek(key),
            Self::ReadOnly(iter) => iter.seek(key),
        }
    }

    /// Returns true if the iterator is positioned at a valid key-value pair.
    fn valid(&self) -> bool {
        match self {
            Self::ReadWrite(iter) => iter.valid(),
            Self::ReadOnly(iter) => iter.valid(),
        }
    }

    /// Returns the current key, if valid.
    fn key(&self) -> Option<&[u8]> {
        match self {
            Self::ReadWrite(iter) => iter.key(),
            Self::ReadOnly(iter) => iter.key(),
        }
    }

    /// Returns the current value, if valid.
    fn value(&self) -> Option<&[u8]> {
        match self {
            Self::ReadWrite(iter) => iter.value(),
            Self::ReadOnly(iter) => iter.value(),
        }
    }

    /// Advances the iterator to the next key.
    fn next(&mut self) {
        match self {
            Self::ReadWrite(iter) => iter.next(),
            Self::ReadOnly(iter) => iter.next(),
        }
    }

    /// Returns the status of the iterator.
    fn status(&self) -> Result<(), rocksdb::Error> {
        match self {
            Self::ReadWrite(iter) => iter.status(),
            Self::ReadOnly(iter) => iter.status(),
        }
    }
}

/// Iterator over a `RocksDB` table (non-transactional).
///
/// Yields decoded `(Key, Value)` pairs in key order.
pub struct RocksDBIter<'db, T: Table> {
    inner: RocksDBIterEnum<'db>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Table> fmt::Debug for RocksDBIter<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBIter").field("table", &T::NAME).finish_non_exhaustive()
    }
}

impl<T: Table> Iterator for RocksDBIter<'_, T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(decode_iter_item::<T>(self.inner.next()?))
    }
}

/// Raw iterator over a `RocksDB` table (non-transactional).
///
/// Yields raw `(key_bytes, value_bytes)` pairs without decoding.
pub struct RocksDBRawIter<'db> {
    inner: RocksDBIterEnum<'db>,
}

impl fmt::Debug for RocksDBRawIter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBRawIter").finish_non_exhaustive()
    }
}

impl Iterator for RocksDBRawIter<'_> {
    type Item = ProviderResult<(Box<[u8]>, Box<[u8]>)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next()? {
            Ok(kv) => Some(Ok(kv)),
            Err(e) => Some(Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            })))),
        }
    }
}

/// Iterator over a `RocksDB` table within a transaction.
///
/// Yields decoded `(Key, Value)` pairs. Sees uncommitted writes.
pub struct RocksTxIter<'tx, T: Table> {
    inner: rocksdb::DBIteratorWithThreadMode<'tx, Transaction<'tx, OptimisticTransactionDB>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Table> fmt::Debug for RocksTxIter<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksTxIter").field("table", &T::NAME).finish_non_exhaustive()
    }
}

impl<T: Table> Iterator for RocksTxIter<'_, T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(decode_iter_item::<T>(self.inner.next()?))
    }
}

/// Decodes a raw key-value pair from a `RocksDB` iterator into typed table entries.
///
/// Handles both error propagation from the underlying iterator and
/// decoding/decompression of the key and value bytes.
fn decode_iter_item<T: Table>(result: RawKVResult) -> ProviderResult<(T::Key, T::Value)> {
    let (key_bytes, value_bytes) = result.map_err(|e| {
        ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
            message: e.to_string().into(),
            code: -1,
        }))
    })?;

    let key = <T::Key as reth_db_api::table::Decode>::decode(&key_bytes)
        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

    let value = T::Value::decompress(&value_bytes)
        .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

    Ok((key, value))
}

/// Converts Reth's [`LogLevel`] to `RocksDB`'s [`rocksdb::LogLevel`].
const fn convert_log_level(level: LogLevel) -> rocksdb::LogLevel {
    match level {
        LogLevel::Fatal => rocksdb::LogLevel::Fatal,
        LogLevel::Error => rocksdb::LogLevel::Error,
        LogLevel::Warn => rocksdb::LogLevel::Warn,
        LogLevel::Notice | LogLevel::Verbose => rocksdb::LogLevel::Info,
        LogLevel::Debug | LogLevel::Trace | LogLevel::Extra => rocksdb::LogLevel::Debug,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::HistoryInfo;
    use alloy_primitives::{Address, TxHash, B256};
    use reth_db_api::{
        models::{
            sharded_key::{ShardedKey, NUM_OF_INDICES_IN_SHARD},
            storage_sharded_key::StorageShardedKey,
            IntegerList,
        },
        table::Table,
        tables,
    };
    use tempfile::TempDir;

    #[test]
    fn test_with_default_tables_registers_required_column_families() {
        let temp_dir = TempDir::new().unwrap();

        // Build with default tables
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        // Should be able to write/read TransactionHashNumbers
        let tx_hash = TxHash::from(B256::from([1u8; 32]));
        provider.put::<tables::TransactionHashNumbers>(tx_hash, &100).unwrap();
        assert_eq!(provider.get::<tables::TransactionHashNumbers>(tx_hash).unwrap(), Some(100));

        // Should be able to write/read AccountsHistory
        let key = ShardedKey::new(Address::ZERO, 100);
        let value = IntegerList::default();
        provider.put::<tables::AccountsHistory>(key.clone(), &value).unwrap();
        assert!(provider.get::<tables::AccountsHistory>(key).unwrap().is_some());

        // Should be able to write/read StoragesHistory
        let key = StorageShardedKey::new(Address::ZERO, B256::ZERO, 100);
        provider.put::<tables::StoragesHistory>(key.clone(), &value).unwrap();
        assert!(provider.get::<tables::StoragesHistory>(key).unwrap().is_some());
    }

    #[derive(Debug)]
    struct TestTable;

    impl Table for TestTable {
        const NAME: &'static str = "TestTable";
        const DUPSORT: bool = false;
        type Key = u64;
        type Value = Vec<u8>;
    }

    #[test]
    fn test_basic_operations() {
        let temp_dir = TempDir::new().unwrap();

        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<TestTable>() // Type-safe!
            .build()
            .unwrap();

        let key = 42u64;
        let value = b"test_value".to_vec();

        // Test write
        provider.put::<TestTable>(key, &value).unwrap();

        // Test read
        let result = provider.get::<TestTable>(key).unwrap();
        assert_eq!(result, Some(value));

        // Test delete
        provider.delete::<TestTable>(key).unwrap();

        // Verify deletion
        assert_eq!(provider.get::<TestTable>(key).unwrap(), None);
    }

    #[test]
    fn test_batch_operations() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Write multiple entries in a batch
        provider
            .write_batch(|batch| {
                for i in 0..10u64 {
                    let value = format!("value_{i}").into_bytes();
                    batch.put::<TestTable>(i, &value)?;
                }
                Ok(())
            })
            .unwrap();

        // Read all entries
        for i in 0..10u64 {
            let value = format!("value_{i}").into_bytes();
            assert_eq!(provider.get::<TestTable>(i).unwrap(), Some(value));
        }

        // Delete all entries in a batch
        provider
            .write_batch(|batch| {
                for i in 0..10u64 {
                    batch.delete::<TestTable>(i)?;
                }
                Ok(())
            })
            .unwrap();

        // Verify all deleted
        for i in 0..10u64 {
            assert_eq!(provider.get::<TestTable>(i).unwrap(), None);
        }
    }

    #[test]
    fn test_with_real_table() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .with_metrics()
            .build()
            .unwrap();

        let tx_hash = TxHash::from(B256::from([1u8; 32]));

        // Insert and retrieve
        provider.put::<tables::TransactionHashNumbers>(tx_hash, &100).unwrap();
        assert_eq!(provider.get::<tables::TransactionHashNumbers>(tx_hash).unwrap(), Some(100));

        // Batch insert multiple transactions
        provider
            .write_batch(|batch| {
                for i in 0..10u64 {
                    let hash = TxHash::from(B256::from([i as u8; 32]));
                    let value = i * 100;
                    batch.put::<tables::TransactionHashNumbers>(hash, &value)?;
                }
                Ok(())
            })
            .unwrap();

        // Verify batch insertions
        for i in 0..10u64 {
            let hash = TxHash::from(B256::from([i as u8; 32]));
            assert_eq!(
                provider.get::<tables::TransactionHashNumbers>(hash).unwrap(),
                Some(i * 100)
            );
        }
    }
    #[test]
    fn test_statistics_enabled() {
        let temp_dir = TempDir::new().unwrap();
        // Just verify that building with statistics doesn't panic
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<TestTable>()
            .with_statistics()
            .build()
            .unwrap();

        // Do operations - data should be immediately readable with OptimisticTransactionDB
        for i in 0..10 {
            let value = vec![i as u8];
            provider.put::<TestTable>(i, &value).unwrap();
            // Verify write is visible
            assert_eq!(provider.get::<TestTable>(i).unwrap(), Some(value));
        }
    }

    #[test]
    fn test_data_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert data - OptimisticTransactionDB writes are immediately visible
        let value = vec![42u8; 1000];
        for i in 0..100 {
            provider.put::<TestTable>(i, &value).unwrap();
        }

        // Verify data is readable
        for i in 0..100 {
            assert!(provider.get::<TestTable>(i).unwrap().is_some(), "Data should be readable");
        }
    }

    #[test]
    fn test_transaction_read_your_writes() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Create a transaction
        let tx = provider.tx();

        // Write data within the transaction
        let key = 42u64;
        let value = b"test_value".to_vec();
        tx.put::<TestTable>(key, &value).unwrap();

        // Read-your-writes: should see uncommitted data in same transaction
        let result = tx.get::<TestTable>(key).unwrap();
        assert_eq!(
            result,
            Some(value.clone()),
            "Transaction should see its own uncommitted writes"
        );

        // Data should NOT be visible via provider (outside transaction)
        let provider_result = provider.get::<TestTable>(key).unwrap();
        assert_eq!(provider_result, None, "Uncommitted data should not be visible outside tx");

        // Commit the transaction
        tx.commit().unwrap();

        // Now data should be visible via provider
        let committed_result = provider.get::<TestTable>(key).unwrap();
        assert_eq!(committed_result, Some(value), "Committed data should be visible");
    }

    #[test]
    fn test_transaction_rollback() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // First, put some initial data
        let key = 100u64;
        let initial_value = b"initial".to_vec();
        provider.put::<TestTable>(key, &initial_value).unwrap();

        // Create a transaction and modify data
        let tx = provider.tx();
        let new_value = b"modified".to_vec();
        tx.put::<TestTable>(key, &new_value).unwrap();

        // Verify modification is visible within transaction
        assert_eq!(tx.get::<TestTable>(key).unwrap(), Some(new_value));

        // Rollback instead of commit
        tx.rollback().unwrap();

        // Data should be unchanged (initial value)
        let result = provider.get::<TestTable>(key).unwrap();
        assert_eq!(result, Some(initial_value), "Rollback should preserve original data");
    }

    #[test]
    fn test_transaction_iterator() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Create a transaction
        let tx = provider.tx();

        // Write multiple entries
        for i in 0..5u64 {
            let value = format!("value_{i}").into_bytes();
            tx.put::<TestTable>(i, &value).unwrap();
        }

        // Iterate - should see uncommitted writes
        let mut count = 0;
        for result in tx.iter::<TestTable>().unwrap() {
            let (key, value) = result.unwrap();
            assert_eq!(value, format!("value_{key}").into_bytes());
            count += 1;
        }
        assert_eq!(count, 5, "Iterator should see all uncommitted writes");

        // Commit
        tx.commit().unwrap();
    }

    #[test]
    fn test_batch_manual_commit() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Create a batch via provider.batch()
        let mut batch = provider.batch();

        // Add entries
        for i in 0..10u64 {
            let value = format!("batch_value_{i}").into_bytes();
            batch.put::<TestTable>(i, &value).unwrap();
        }

        // Verify len/is_empty
        assert_eq!(batch.len(), 10);
        assert!(!batch.is_empty());

        // Data should NOT be visible before commit
        assert_eq!(provider.get::<TestTable>(0).unwrap(), None);

        // Commit the batch
        batch.commit().unwrap();

        // Now data should be visible
        for i in 0..10u64 {
            let value = format!("batch_value_{i}").into_bytes();
            assert_eq!(provider.get::<TestTable>(i).unwrap(), Some(value));
        }
    }

    #[test]
    fn test_first_and_last_entry() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Empty table should return None for both
        assert_eq!(provider.first::<TestTable>().unwrap(), None);
        assert_eq!(provider.last::<TestTable>().unwrap(), None);

        // Insert some entries
        provider.put::<TestTable>(10, &b"value_10".to_vec()).unwrap();
        provider.put::<TestTable>(20, &b"value_20".to_vec()).unwrap();
        provider.put::<TestTable>(5, &b"value_5".to_vec()).unwrap();

        // First should return the smallest key
        let first = provider.first::<TestTable>().unwrap();
        assert_eq!(first, Some((5, b"value_5".to_vec())));

        // Last should return the largest key
        let last = provider.last::<TestTable>().unwrap();
        assert_eq!(last, Some((20, b"value_20".to_vec())));
    }

    /// Tests the edge case where block < `lowest_available_block_number`.
    /// This case cannot be tested via `HistoricalStateProviderRef` (which errors before lookup),
    /// so we keep this RocksDB-specific test to verify the low-level behavior.
    #[test]
    fn test_account_history_info_pruned_before_first_entry() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create a single shard starting at block 100
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = ShardedKey::new(address, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query for block 50 with lowest_available_block_number = 100
        // This simulates a pruned state where data before block 100 is not available.
        // Since we're before the first write AND pruning boundary is set, we need to
        // check the changeset at the first write block.
        let result = tx.account_history_info(address, 50, Some(100)).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(100));

        tx.rollback().unwrap();
    }

    #[test]
    fn test_account_history_shard_split_at_boundary() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let limit = NUM_OF_INDICES_IN_SHARD;

        // Add exactly NUM_OF_INDICES_IN_SHARD + 1 indices to trigger a split
        let indices: Vec<u64> = (0..=(limit as u64)).collect();
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, indices).unwrap();
        batch.commit().unwrap();

        // Should have 2 shards: one completed shard and one sentinel shard
        let completed_key = ShardedKey::new(address, (limit - 1) as u64);
        let sentinel_key = ShardedKey::new(address, u64::MAX);

        let completed_shard = provider.get::<tables::AccountsHistory>(completed_key).unwrap();
        let sentinel_shard = provider.get::<tables::AccountsHistory>(sentinel_key).unwrap();

        assert!(completed_shard.is_some(), "completed shard should exist");
        assert!(sentinel_shard.is_some(), "sentinel shard should exist");

        let completed_shard = completed_shard.unwrap();
        let sentinel_shard = sentinel_shard.unwrap();

        assert_eq!(completed_shard.len(), limit as u64, "completed shard should be full");
        assert_eq!(sentinel_shard.len(), 1, "sentinel shard should have 1 element");
    }

    #[test]
    fn test_account_history_multiple_shard_splits() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x43; 20]);
        let limit = NUM_OF_INDICES_IN_SHARD;

        // First batch: add NUM_OF_INDICES_IN_SHARD indices
        let first_batch_indices: Vec<u64> = (0..limit as u64).collect();
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, first_batch_indices).unwrap();
        batch.commit().unwrap();

        // Should have just a sentinel shard (exactly at limit, not over)
        let sentinel_key = ShardedKey::new(address, u64::MAX);
        let shard = provider.get::<tables::AccountsHistory>(sentinel_key.clone()).unwrap();
        assert!(shard.is_some());
        assert_eq!(shard.unwrap().len(), limit as u64);

        // Second batch: add another NUM_OF_INDICES_IN_SHARD + 1 indices (causing 2 more shards)
        let second_batch_indices: Vec<u64> = (limit as u64..=(2 * limit) as u64).collect();
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, second_batch_indices).unwrap();
        batch.commit().unwrap();

        // Now we should have: 2 completed shards + 1 sentinel shard
        let first_completed = ShardedKey::new(address, (limit - 1) as u64);
        let second_completed = ShardedKey::new(address, (2 * limit - 1) as u64);

        assert!(
            provider.get::<tables::AccountsHistory>(first_completed).unwrap().is_some(),
            "first completed shard should exist"
        );
        assert!(
            provider.get::<tables::AccountsHistory>(second_completed).unwrap().is_some(),
            "second completed shard should exist"
        );
        assert!(
            provider.get::<tables::AccountsHistory>(sentinel_key).unwrap().is_some(),
            "sentinel shard should exist"
        );
    }

    #[test]
    fn test_storage_history_shard_split_at_boundary() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x44; 20]);
        let slot = B256::from([0x55; 32]);
        let limit = NUM_OF_INDICES_IN_SHARD;

        // Add exactly NUM_OF_INDICES_IN_SHARD + 1 indices to trigger a split
        let indices: Vec<u64> = (0..=(limit as u64)).collect();
        let mut batch = provider.batch();
        batch.append_storage_history_shard(address, slot, indices).unwrap();
        batch.commit().unwrap();

        // Should have 2 shards: one completed shard and one sentinel shard
        let completed_key = StorageShardedKey::new(address, slot, (limit - 1) as u64);
        let sentinel_key = StorageShardedKey::new(address, slot, u64::MAX);

        let completed_shard = provider.get::<tables::StoragesHistory>(completed_key).unwrap();
        let sentinel_shard = provider.get::<tables::StoragesHistory>(sentinel_key).unwrap();

        assert!(completed_shard.is_some(), "completed shard should exist");
        assert!(sentinel_shard.is_some(), "sentinel shard should exist");

        let completed_shard = completed_shard.unwrap();
        let sentinel_shard = sentinel_shard.unwrap();

        assert_eq!(completed_shard.len(), limit as u64, "completed shard should be full");
        assert_eq!(sentinel_shard.len(), 1, "sentinel shard should have 1 element");
    }

    #[test]
    fn test_storage_history_multiple_shard_splits() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x46; 20]);
        let slot = B256::from([0x57; 32]);
        let limit = NUM_OF_INDICES_IN_SHARD;

        // First batch: add NUM_OF_INDICES_IN_SHARD indices
        let first_batch_indices: Vec<u64> = (0..limit as u64).collect();
        let mut batch = provider.batch();
        batch.append_storage_history_shard(address, slot, first_batch_indices).unwrap();
        batch.commit().unwrap();

        // Should have just a sentinel shard (exactly at limit, not over)
        let sentinel_key = StorageShardedKey::new(address, slot, u64::MAX);
        let shard = provider.get::<tables::StoragesHistory>(sentinel_key.clone()).unwrap();
        assert!(shard.is_some());
        assert_eq!(shard.unwrap().len(), limit as u64);

        // Second batch: add another NUM_OF_INDICES_IN_SHARD + 1 indices (causing 2 more shards)
        let second_batch_indices: Vec<u64> = (limit as u64..=(2 * limit) as u64).collect();
        let mut batch = provider.batch();
        batch.append_storage_history_shard(address, slot, second_batch_indices).unwrap();
        batch.commit().unwrap();

        // Now we should have: 2 completed shards + 1 sentinel shard
        let first_completed = StorageShardedKey::new(address, slot, (limit - 1) as u64);
        let second_completed = StorageShardedKey::new(address, slot, (2 * limit - 1) as u64);

        assert!(
            provider.get::<tables::StoragesHistory>(first_completed).unwrap().is_some(),
            "first completed shard should exist"
        );
        assert!(
            provider.get::<tables::StoragesHistory>(second_completed).unwrap().is_some(),
            "second completed shard should exist"
        );
        assert!(
            provider.get::<tables::StoragesHistory>(sentinel_key).unwrap().is_some(),
            "sentinel shard should exist"
        );
    }

    #[test]
    fn test_clear_table() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let key = ShardedKey::new(address, u64::MAX);
        let blocks = BlockNumberList::new_pre_sorted([1, 2, 3]);

        provider.put::<tables::AccountsHistory>(key.clone(), &blocks).unwrap();
        assert!(provider.get::<tables::AccountsHistory>(key.clone()).unwrap().is_some());

        provider.clear::<tables::AccountsHistory>().unwrap();

        assert!(
            provider.get::<tables::AccountsHistory>(key).unwrap().is_none(),
            "table should be empty after clear"
        );
        assert!(
            provider.first::<tables::AccountsHistory>().unwrap().is_none(),
            "first() should return None after clear"
        );
    }

    #[test]
    fn test_clear_empty_table() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        assert!(provider.first::<tables::AccountsHistory>().unwrap().is_none());

        provider.clear::<tables::AccountsHistory>().unwrap();

        assert!(provider.first::<tables::AccountsHistory>().unwrap().is_none());
    }

    #[test]
    fn test_unwind_account_history_to_basic() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Add blocks 0-10
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 0..=10).unwrap();
        batch.commit().unwrap();

        // Verify we have blocks 0-10
        let key = ShardedKey::new(address, u64::MAX);
        let result = provider.get::<tables::AccountsHistory>(key.clone()).unwrap();
        assert!(result.is_some());
        let blocks: Vec<u64> = result.unwrap().iter().collect();
        assert_eq!(blocks, (0..=10).collect::<Vec<_>>());

        // Unwind to block 5 (keep blocks 0-5, remove 6-10)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 5).unwrap();
        batch.commit().unwrap();

        // Verify only blocks 0-5 remain
        let result = provider.get::<tables::AccountsHistory>(key).unwrap();
        assert!(result.is_some());
        let blocks: Vec<u64> = result.unwrap().iter().collect();
        assert_eq!(blocks, (0..=5).collect::<Vec<_>>());
    }

    #[test]
    fn test_unwind_account_history_to_removes_all() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Add blocks 5-10
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 5..=10).unwrap();
        batch.commit().unwrap();

        // Unwind to block 4 (removes all blocks since they're all > 4)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 4).unwrap();
        batch.commit().unwrap();

        // Verify no data remains for this address
        let key = ShardedKey::new(address, u64::MAX);
        let result = provider.get::<tables::AccountsHistory>(key).unwrap();
        assert!(result.is_none(), "Should have no data after full unwind");
    }

    #[test]
    fn test_unwind_account_history_to_no_op() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Add blocks 0-5
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 0..=5).unwrap();
        batch.commit().unwrap();

        // Unwind to block 10 (no-op since all blocks are <= 10)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 10).unwrap();
        batch.commit().unwrap();

        // Verify blocks 0-5 still remain
        let key = ShardedKey::new(address, u64::MAX);
        let result = provider.get::<tables::AccountsHistory>(key).unwrap();
        assert!(result.is_some());
        let blocks: Vec<u64> = result.unwrap().iter().collect();
        assert_eq!(blocks, (0..=5).collect::<Vec<_>>());
    }

    #[test]
    fn test_unwind_account_history_to_block_zero() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Add blocks 0-5 (including block 0)
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 0..=5).unwrap();
        batch.commit().unwrap();

        // Unwind to block 0 (keep only block 0, remove 1-5)
        // This simulates the caller doing: unwind_to = min_block.checked_sub(1) where min_block = 1
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 0).unwrap();
        batch.commit().unwrap();

        // Verify only block 0 remains
        let key = ShardedKey::new(address, u64::MAX);
        let result = provider.get::<tables::AccountsHistory>(key).unwrap();
        assert!(result.is_some());
        let blocks: Vec<u64> = result.unwrap().iter().collect();
        assert_eq!(blocks, vec![0]);
    }

    #[test]
    fn test_unwind_account_history_to_multi_shard() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create multiple shards by adding more than NUM_OF_INDICES_IN_SHARD entries
        // For testing, we'll manually create shards with specific keys
        let mut batch = provider.batch();

        // First shard: blocks 1-50, keyed by 50
        let shard1 = BlockNumberList::new_pre_sorted(1..=50);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, 50), &shard1).unwrap();

        // Second shard: blocks 51-100, keyed by MAX (sentinel)
        let shard2 = BlockNumberList::new_pre_sorted(51..=100);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, u64::MAX), &shard2).unwrap();

        batch.commit().unwrap();

        // Verify we have 2 shards
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 2);

        // Unwind to block 75 (keep 1-75, remove 76-100)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 75).unwrap();
        batch.commit().unwrap();

        // Verify: shard1 should be untouched, shard2 should be truncated
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 2);

        // First shard unchanged
        assert_eq!(shards[0].0.highest_block_number, 50);
        assert_eq!(shards[0].1.iter().collect::<Vec<_>>(), (1..=50).collect::<Vec<_>>());

        // Second shard truncated and re-keyed to MAX
        assert_eq!(shards[1].0.highest_block_number, u64::MAX);
        assert_eq!(shards[1].1.iter().collect::<Vec<_>>(), (51..=75).collect::<Vec<_>>());
    }

    #[test]
    fn test_unwind_account_history_to_multi_shard_boundary_empty() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create two shards
        let mut batch = provider.batch();

        // First shard: blocks 1-50, keyed by 50
        let shard1 = BlockNumberList::new_pre_sorted(1..=50);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, 50), &shard1).unwrap();

        // Second shard: blocks 75-100, keyed by MAX
        let shard2 = BlockNumberList::new_pre_sorted(75..=100);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, u64::MAX), &shard2).unwrap();

        batch.commit().unwrap();

        // Unwind to block 60 (removes all of shard2 since 75 > 60, promotes shard1 to MAX)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 60).unwrap();
        batch.commit().unwrap();

        // Verify: only shard1 remains, now keyed as MAX
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].0.highest_block_number, u64::MAX);
        assert_eq!(shards[0].1.iter().collect::<Vec<_>>(), (1..=50).collect::<Vec<_>>());
    }

    #[test]
    fn test_account_history_shards_iterator() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let other_address = Address::from([0x43; 20]);

        // Add data for two addresses
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 0..=5).unwrap();
        batch.append_account_history_shard(other_address, 10..=15).unwrap();
        batch.commit().unwrap();

        // Query shards for first address only
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].0.key, address);

        // Query shards for second address only
        let shards = provider.account_history_shards(other_address).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].0.key, other_address);

        // Query shards for non-existent address
        let non_existent = Address::from([0x99; 20]);
        let shards = provider.account_history_shards(non_existent).unwrap();
        assert!(shards.is_empty());
    }

    #[test]
    fn test_clear_account_history() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Add blocks 0-10
        let mut batch = provider.batch();
        batch.append_account_history_shard(address, 0..=10).unwrap();
        batch.commit().unwrap();

        // Clear all history (simulates unwind from block 0)
        let mut batch = provider.batch();
        batch.clear_account_history(address).unwrap();
        batch.commit().unwrap();

        // Verify no data remains
        let shards = provider.account_history_shards(address).unwrap();
        assert!(shards.is_empty(), "All shards should be deleted");
    }

    #[test]
    fn test_unwind_non_sentinel_boundary() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create three shards with non-sentinel boundary
        let mut batch = provider.batch();

        // Shard 1: blocks 1-50, keyed by 50
        let shard1 = BlockNumberList::new_pre_sorted(1..=50);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, 50), &shard1).unwrap();

        // Shard 2: blocks 51-100, keyed by 100 (non-sentinel, will be boundary)
        let shard2 = BlockNumberList::new_pre_sorted(51..=100);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, 100), &shard2).unwrap();

        // Shard 3: blocks 101-150, keyed by MAX (will be deleted)
        let shard3 = BlockNumberList::new_pre_sorted(101..=150);
        batch.put::<tables::AccountsHistory>(ShardedKey::new(address, u64::MAX), &shard3).unwrap();

        batch.commit().unwrap();

        // Verify 3 shards
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 3);

        // Unwind to block 75 (truncates shard2, deletes shard3)
        let mut batch = provider.batch();
        batch.unwind_account_history_to(address, 75).unwrap();
        batch.commit().unwrap();

        // Verify: shard1 unchanged, shard2 truncated and re-keyed to MAX, shard3 deleted
        let shards = provider.account_history_shards(address).unwrap();
        assert_eq!(shards.len(), 2);

        // First shard unchanged
        assert_eq!(shards[0].0.highest_block_number, 50);
        assert_eq!(shards[0].1.iter().collect::<Vec<_>>(), (1..=50).collect::<Vec<_>>());

        // Second shard truncated and re-keyed to MAX
        assert_eq!(shards[1].0.highest_block_number, u64::MAX);
        assert_eq!(shards[1].1.iter().collect::<Vec<_>>(), (51..=75).collect::<Vec<_>>());
    }

    #[test]
    fn test_batch_auto_commit_on_threshold() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Create batch with tiny threshold (1KB) to force auto-commits
        let mut batch = RocksDBBatch {
            provider: &provider,
            inner: WriteBatchWithTransaction::<true>::default(),
            buf: Vec::new(),
            auto_commit_threshold: Some(1024), // 1KB
        };

        // Write entries until we exceed threshold multiple times
        // Each entry is ~20 bytes, so 100 entries = ~2KB = 2 auto-commits
        for i in 0..100u64 {
            let value = format!("value_{i:04}").into_bytes();
            batch.put::<TestTable>(i, &value).unwrap();
        }

        // Data should already be visible (auto-committed) even before final commit
        // At least some entries should be readable
        let first_visible = provider.get::<TestTable>(0).unwrap();
        assert!(first_visible.is_some(), "Auto-committed data should be visible");

        // Final commit for remaining batch
        batch.commit().unwrap();

        // All entries should now be visible
        for i in 0..100u64 {
            let value = format!("value_{i:04}").into_bytes();
            assert_eq!(provider.get::<TestTable>(i).unwrap(), Some(value));
        }
    }

    // ==================== PARAMETERIZED PRUNE TESTS ====================

    /// Test case for account history pruning
    struct AccountPruneCase {
        name: &'static str,
        initial_shards: &'static [(u64, &'static [u64])],
        prune_to: u64,
        expected_outcome: PruneShardOutcome,
        expected_shards: &'static [(u64, &'static [u64])],
    }

    /// Test case for storage history pruning
    struct StoragePruneCase {
        name: &'static str,
        initial_shards: &'static [(u64, &'static [u64])],
        prune_to: u64,
        expected_outcome: PruneShardOutcome,
        expected_shards: &'static [(u64, &'static [u64])],
    }

    #[test]
    fn test_prune_account_history_cases() {
        const MAX: u64 = u64::MAX;
        const CASES: &[AccountPruneCase] = &[
            AccountPruneCase {
                name: "single_shard_truncate",
                initial_shards: &[(MAX, &[10, 20, 30, 40])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[30, 40])],
            },
            AccountPruneCase {
                name: "single_shard_delete_all",
                initial_shards: &[(MAX, &[10, 20])],
                prune_to: 20,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[],
            },
            AccountPruneCase {
                name: "single_shard_noop",
                initial_shards: &[(MAX, &[10, 20])],
                prune_to: 5,
                expected_outcome: PruneShardOutcome::Unchanged,
                expected_shards: &[(MAX, &[10, 20])],
            },
            AccountPruneCase {
                name: "no_shards",
                initial_shards: &[],
                prune_to: 100,
                expected_outcome: PruneShardOutcome::Unchanged,
                expected_shards: &[],
            },
            AccountPruneCase {
                name: "multi_shard_truncate_first",
                initial_shards: &[(30, &[10, 20, 30]), (MAX, &[40, 50, 60])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(30, &[30]), (MAX, &[40, 50, 60])],
            },
            AccountPruneCase {
                name: "delete_first_shard_sentinel_unchanged",
                initial_shards: &[(20, &[10, 20]), (MAX, &[30, 40])],
                prune_to: 20,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[30, 40])],
            },
            AccountPruneCase {
                name: "multi_shard_delete_all_but_last",
                initial_shards: &[(10, &[5, 10]), (20, &[15, 20]), (MAX, &[25, 30])],
                prune_to: 22,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[25, 30])],
            },
            AccountPruneCase {
                name: "mid_shard_preserves_key",
                initial_shards: &[(50, &[10, 20, 30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(50, &[30, 40, 50]), (MAX, &[60, 70])],
            },
            // Equivalence tests
            AccountPruneCase {
                name: "equiv_delete_early_shards_keep_sentinel",
                initial_shards: &[(20, &[10, 15, 20]), (50, &[30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 55,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[60, 70])],
            },
            AccountPruneCase {
                name: "equiv_sentinel_becomes_empty_with_prev",
                initial_shards: &[(50, &[30, 40, 50]), (MAX, &[35])],
                prune_to: 40,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[50])],
            },
            AccountPruneCase {
                name: "equiv_all_shards_become_empty",
                initial_shards: &[(50, &[30, 40, 50]), (MAX, &[51])],
                prune_to: 51,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[],
            },
            AccountPruneCase {
                name: "equiv_non_sentinel_last_shard_promoted",
                initial_shards: &[(100, &[50, 75, 100])],
                prune_to: 60,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[75, 100])],
            },
            AccountPruneCase {
                name: "equiv_filter_within_shard",
                initial_shards: &[(MAX, &[10, 20, 30, 40])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[30, 40])],
            },
            AccountPruneCase {
                name: "equiv_multi_shard_partial_delete",
                initial_shards: &[(20, &[10, 20]), (50, &[30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 35,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(50, &[40, 50]), (MAX, &[60, 70])],
            },
        ];

        let address = Address::from([0x42; 20]);

        for case in CASES {
            let temp_dir = TempDir::new().unwrap();
            let provider =
                RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

            // Setup initial shards
            let mut batch = provider.batch();
            for (highest, blocks) in case.initial_shards {
                let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                batch
                    .put::<tables::AccountsHistory>(ShardedKey::new(address, *highest), &shard)
                    .unwrap();
            }
            batch.commit().unwrap();

            // Prune
            let mut batch = provider.batch();
            let outcome = batch.prune_account_history_to(address, case.prune_to).unwrap();
            batch.commit().unwrap();

            // Assert outcome
            assert_eq!(outcome, case.expected_outcome, "case '{}': wrong outcome", case.name);

            // Assert final shards
            let shards = provider.account_history_shards(address).unwrap();
            assert_eq!(
                shards.len(),
                case.expected_shards.len(),
                "case '{}': wrong shard count",
                case.name
            );
            for (i, ((key, blocks), (exp_key, exp_blocks))) in
                shards.iter().zip(case.expected_shards.iter()).enumerate()
            {
                assert_eq!(
                    key.highest_block_number, *exp_key,
                    "case '{}': shard {} wrong key",
                    case.name, i
                );
                assert_eq!(
                    blocks.iter().collect::<Vec<_>>(),
                    *exp_blocks,
                    "case '{}': shard {} wrong blocks",
                    case.name,
                    i
                );
            }
        }
    }

    #[test]
    fn test_prune_storage_history_cases() {
        const MAX: u64 = u64::MAX;
        const CASES: &[StoragePruneCase] = &[
            StoragePruneCase {
                name: "single_shard_truncate",
                initial_shards: &[(MAX, &[10, 20, 30, 40])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[30, 40])],
            },
            StoragePruneCase {
                name: "single_shard_delete_all",
                initial_shards: &[(MAX, &[10, 20])],
                prune_to: 20,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[],
            },
            StoragePruneCase {
                name: "noop",
                initial_shards: &[(MAX, &[10, 20])],
                prune_to: 5,
                expected_outcome: PruneShardOutcome::Unchanged,
                expected_shards: &[(MAX, &[10, 20])],
            },
            StoragePruneCase {
                name: "no_shards",
                initial_shards: &[],
                prune_to: 100,
                expected_outcome: PruneShardOutcome::Unchanged,
                expected_shards: &[],
            },
            StoragePruneCase {
                name: "mid_shard_preserves_key",
                initial_shards: &[(50, &[10, 20, 30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(50, &[30, 40, 50]), (MAX, &[60, 70])],
            },
            // Equivalence tests
            StoragePruneCase {
                name: "equiv_sentinel_promotion",
                initial_shards: &[(100, &[50, 75, 100])],
                prune_to: 60,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[75, 100])],
            },
            StoragePruneCase {
                name: "equiv_delete_early_shards_keep_sentinel",
                initial_shards: &[(20, &[10, 15, 20]), (50, &[30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 55,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[60, 70])],
            },
            StoragePruneCase {
                name: "equiv_sentinel_becomes_empty_with_prev",
                initial_shards: &[(50, &[30, 40, 50]), (MAX, &[35])],
                prune_to: 40,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(MAX, &[50])],
            },
            StoragePruneCase {
                name: "equiv_all_shards_become_empty",
                initial_shards: &[(50, &[30, 40, 50]), (MAX, &[51])],
                prune_to: 51,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[],
            },
            StoragePruneCase {
                name: "equiv_filter_within_shard",
                initial_shards: &[(MAX, &[10, 20, 30, 40])],
                prune_to: 25,
                expected_outcome: PruneShardOutcome::Updated,
                expected_shards: &[(MAX, &[30, 40])],
            },
            StoragePruneCase {
                name: "equiv_multi_shard_partial_delete",
                initial_shards: &[(20, &[10, 20]), (50, &[30, 40, 50]), (MAX, &[60, 70])],
                prune_to: 35,
                expected_outcome: PruneShardOutcome::Deleted,
                expected_shards: &[(50, &[40, 50]), (MAX, &[60, 70])],
            },
        ];

        let address = Address::from([0x42; 20]);
        let storage_key = B256::from([0x01; 32]);

        for case in CASES {
            let temp_dir = TempDir::new().unwrap();
            let provider =
                RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

            // Setup initial shards
            let mut batch = provider.batch();
            for (highest, blocks) in case.initial_shards {
                let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                let key = if *highest == MAX {
                    StorageShardedKey::last(address, storage_key)
                } else {
                    StorageShardedKey::new(address, storage_key, *highest)
                };
                batch.put::<tables::StoragesHistory>(key, &shard).unwrap();
            }
            batch.commit().unwrap();

            // Prune
            let mut batch = provider.batch();
            let outcome =
                batch.prune_storage_history_to(address, storage_key, case.prune_to).unwrap();
            batch.commit().unwrap();

            // Assert outcome
            assert_eq!(outcome, case.expected_outcome, "case '{}': wrong outcome", case.name);

            // Assert final shards
            let shards = provider.storage_history_shards(address, storage_key).unwrap();
            assert_eq!(
                shards.len(),
                case.expected_shards.len(),
                "case '{}': wrong shard count",
                case.name
            );
            for (i, ((key, blocks), (exp_key, exp_blocks))) in
                shards.iter().zip(case.expected_shards.iter()).enumerate()
            {
                assert_eq!(
                    key.sharded_key.highest_block_number, *exp_key,
                    "case '{}': shard {} wrong key",
                    case.name, i
                );
                assert_eq!(
                    blocks.iter().collect::<Vec<_>>(),
                    *exp_blocks,
                    "case '{}': shard {} wrong blocks",
                    case.name,
                    i
                );
            }
        }
    }

    #[test]
    fn test_prune_storage_history_does_not_affect_other_slots() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let slot1 = B256::from([0x01; 32]);
        let slot2 = B256::from([0x02; 32]);

        // Two different storage slots
        let mut batch = provider.batch();
        batch
            .put::<tables::StoragesHistory>(
                StorageShardedKey::last(address, slot1),
                &BlockNumberList::new_pre_sorted([10u64, 20]),
            )
            .unwrap();
        batch
            .put::<tables::StoragesHistory>(
                StorageShardedKey::last(address, slot2),
                &BlockNumberList::new_pre_sorted([30u64, 40]),
            )
            .unwrap();
        batch.commit().unwrap();

        // Prune slot1 to block 20 (deletes all)
        let mut batch = provider.batch();
        let outcome = batch.prune_storage_history_to(address, slot1, 20).unwrap();
        batch.commit().unwrap();

        assert_eq!(outcome, PruneShardOutcome::Deleted);

        // slot1 should be empty
        let shards1 = provider.storage_history_shards(address, slot1).unwrap();
        assert!(shards1.is_empty());

        // slot2 should be unchanged
        let shards2 = provider.storage_history_shards(address, slot2).unwrap();
        assert_eq!(shards2.len(), 1);
        assert_eq!(shards2[0].1.iter().collect::<Vec<_>>(), vec![30, 40]);
    }

    #[test]
    fn test_prune_invariants() {
        // Test invariants: no empty shards, sentinel is always last
        let address = Address::from([0x42; 20]);
        let storage_key = B256::from([0x01; 32]);

        // Test cases that exercise invariants
        #[allow(clippy::type_complexity)]
        let invariant_cases: &[(&[(u64, &[u64])], u64)] = &[
            // Account: shards where middle becomes empty
            (&[(10, &[5, 10]), (20, &[15, 20]), (u64::MAX, &[25, 30])], 20),
            // Account: non-sentinel shard only, partial prune -> must become sentinel
            (&[(100, &[50, 100])], 60),
        ];

        for (initial_shards, prune_to) in invariant_cases {
            // Test account history invariants
            {
                let temp_dir = TempDir::new().unwrap();
                let provider =
                    RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

                let mut batch = provider.batch();
                for (highest, blocks) in *initial_shards {
                    let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                    batch
                        .put::<tables::AccountsHistory>(ShardedKey::new(address, *highest), &shard)
                        .unwrap();
                }
                batch.commit().unwrap();

                let mut batch = provider.batch();
                batch.prune_account_history_to(address, *prune_to).unwrap();
                batch.commit().unwrap();

                let shards = provider.account_history_shards(address).unwrap();

                // Invariant 1: no empty shards
                for (key, blocks) in &shards {
                    assert!(
                        !blocks.is_empty(),
                        "Account: empty shard at key {}",
                        key.highest_block_number
                    );
                }

                // Invariant 2: last shard is sentinel
                if !shards.is_empty() {
                    let last = shards.last().unwrap();
                    assert_eq!(
                        last.0.highest_block_number,
                        u64::MAX,
                        "Account: last shard must be sentinel"
                    );
                }
            }

            // Test storage history invariants
            {
                let temp_dir = TempDir::new().unwrap();
                let provider =
                    RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

                let mut batch = provider.batch();
                for (highest, blocks) in *initial_shards {
                    let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                    let key = if *highest == u64::MAX {
                        StorageShardedKey::last(address, storage_key)
                    } else {
                        StorageShardedKey::new(address, storage_key, *highest)
                    };
                    batch.put::<tables::StoragesHistory>(key, &shard).unwrap();
                }
                batch.commit().unwrap();

                let mut batch = provider.batch();
                batch.prune_storage_history_to(address, storage_key, *prune_to).unwrap();
                batch.commit().unwrap();

                let shards = provider.storage_history_shards(address, storage_key).unwrap();

                // Invariant 1: no empty shards
                for (key, blocks) in &shards {
                    assert!(
                        !blocks.is_empty(),
                        "Storage: empty shard at key {}",
                        key.sharded_key.highest_block_number
                    );
                }

                // Invariant 2: last shard is sentinel
                if !shards.is_empty() {
                    let last = shards.last().unwrap();
                    assert_eq!(
                        last.0.sharded_key.highest_block_number,
                        u64::MAX,
                        "Storage: last shard must be sentinel"
                    );
                }
            }
        }
    }

    #[test]
    fn test_prune_account_history_batch_multiple_sorted_targets() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let addr1 = Address::from([0x01; 20]);
        let addr2 = Address::from([0x02; 20]);
        let addr3 = Address::from([0x03; 20]);

        // Setup shards for each address
        let mut batch = provider.batch();
        batch
            .put::<tables::AccountsHistory>(
                ShardedKey::new(addr1, u64::MAX),
                &BlockNumberList::new_pre_sorted([10, 20, 30]),
            )
            .unwrap();
        batch
            .put::<tables::AccountsHistory>(
                ShardedKey::new(addr2, u64::MAX),
                &BlockNumberList::new_pre_sorted([5, 10, 15]),
            )
            .unwrap();
        batch
            .put::<tables::AccountsHistory>(
                ShardedKey::new(addr3, u64::MAX),
                &BlockNumberList::new_pre_sorted([100, 200]),
            )
            .unwrap();
        batch.commit().unwrap();

        // Prune all three (sorted by address)
        let mut targets = vec![(addr1, 15), (addr2, 10), (addr3, 50)];
        targets.sort_by_key(|(addr, _)| *addr);

        let mut batch = provider.batch();
        let outcomes = batch.prune_account_history_batch(&targets).unwrap();
        batch.commit().unwrap();

        // addr1: prune <=15, keep [20, 30] -> updated
        // addr2: prune <=10, keep [15] -> updated
        // addr3: prune <=50, keep [100, 200] -> unchanged
        assert_eq!(outcomes.updated, 2);
        assert_eq!(outcomes.unchanged, 1);

        let shards1 = provider.account_history_shards(addr1).unwrap();
        assert_eq!(shards1[0].1.iter().collect::<Vec<_>>(), vec![20, 30]);

        let shards2 = provider.account_history_shards(addr2).unwrap();
        assert_eq!(shards2[0].1.iter().collect::<Vec<_>>(), vec![15]);

        let shards3 = provider.account_history_shards(addr3).unwrap();
        assert_eq!(shards3[0].1.iter().collect::<Vec<_>>(), vec![100, 200]);
    }

    #[test]
    fn test_prune_account_history_batch_target_with_no_shards() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let addr1 = Address::from([0x01; 20]);
        let addr2 = Address::from([0x02; 20]); // No shards for this one
        let addr3 = Address::from([0x03; 20]);

        // Only setup shards for addr1 and addr3
        let mut batch = provider.batch();
        batch
            .put::<tables::AccountsHistory>(
                ShardedKey::new(addr1, u64::MAX),
                &BlockNumberList::new_pre_sorted([10, 20]),
            )
            .unwrap();
        batch
            .put::<tables::AccountsHistory>(
                ShardedKey::new(addr3, u64::MAX),
                &BlockNumberList::new_pre_sorted([30, 40]),
            )
            .unwrap();
        batch.commit().unwrap();

        // Prune all three (addr2 has no shards - tests p > target_prefix case)
        let mut targets = vec![(addr1, 15), (addr2, 100), (addr3, 35)];
        targets.sort_by_key(|(addr, _)| *addr);

        let mut batch = provider.batch();
        let outcomes = batch.prune_account_history_batch(&targets).unwrap();
        batch.commit().unwrap();

        // addr1: updated (keep [20])
        // addr2: unchanged (no shards)
        // addr3: updated (keep [40])
        assert_eq!(outcomes.updated, 2);
        assert_eq!(outcomes.unchanged, 1);

        let shards1 = provider.account_history_shards(addr1).unwrap();
        assert_eq!(shards1[0].1.iter().collect::<Vec<_>>(), vec![20]);

        let shards3 = provider.account_history_shards(addr3).unwrap();
        assert_eq!(shards3[0].1.iter().collect::<Vec<_>>(), vec![40]);
    }

    #[test]
    fn test_prune_storage_history_batch_multiple_sorted_targets() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let addr = Address::from([0x42; 20]);
        let slot1 = B256::from([0x01; 32]);
        let slot2 = B256::from([0x02; 32]);

        // Setup shards
        let mut batch = provider.batch();
        batch
            .put::<tables::StoragesHistory>(
                StorageShardedKey::new(addr, slot1, u64::MAX),
                &BlockNumberList::new_pre_sorted([10, 20, 30]),
            )
            .unwrap();
        batch
            .put::<tables::StoragesHistory>(
                StorageShardedKey::new(addr, slot2, u64::MAX),
                &BlockNumberList::new_pre_sorted([5, 15, 25]),
            )
            .unwrap();
        batch.commit().unwrap();

        // Prune both (sorted)
        let mut targets = vec![((addr, slot1), 15), ((addr, slot2), 10)];
        targets.sort_by_key(|((a, s), _)| (*a, *s));

        let mut batch = provider.batch();
        let outcomes = batch.prune_storage_history_batch(&targets).unwrap();
        batch.commit().unwrap();

        assert_eq!(outcomes.updated, 2);

        let shards1 = provider.storage_history_shards(addr, slot1).unwrap();
        assert_eq!(shards1[0].1.iter().collect::<Vec<_>>(), vec![20, 30]);

        let shards2 = provider.storage_history_shards(addr, slot2).unwrap();
        assert_eq!(shards2[0].1.iter().collect::<Vec<_>>(), vec![15, 25]);
    }
}
