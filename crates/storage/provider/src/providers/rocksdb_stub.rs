//! Stub implementation of `RocksDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when `RocksDB` is not
//! available (either on non-Unix platforms or when the `rocksdb` feature is not enabled).
//! All method calls are cfg-guarded in the calling code, so only type definitions are needed here.

use alloy_primitives::BlockNumber;
use metrics::Label;
use parking_lot::Mutex;
use reth_db_api::{database_metrics::DatabaseMetrics, models::StorageSettings};
use reth_prune_types::PruneMode;
use reth_storage_errors::{db::LogLevel, provider::ProviderResult};
use std::{path::Path, sync::Arc};

/// Pending `RocksDB` batches type alias (stub - uses unit type).
pub(crate) type PendingRocksDBBatches = Arc<Mutex<Vec<()>>>;

/// Statistics for a single `RocksDB` table (column family) - stub.
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

/// Database-level statistics for `RocksDB` - stub.
#[derive(Debug, Clone)]
pub struct RocksDBStats {
    /// Statistics for each table (column family).
    pub tables: Vec<RocksDBTableStats>,
    /// Total size of WAL (Write-Ahead Log) files in bytes.
    pub wal_size_bytes: u64,
}

/// Context for `RocksDB` block writes (stub).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RocksDBWriteCtx {
    /// The first block number being written.
    pub first_block_number: BlockNumber,
    /// The prune mode for transaction lookup, if any.
    pub prune_tx_lookup: Option<PruneMode>,
    /// Storage settings determining what goes to `RocksDB`.
    pub storage_settings: StorageSettings,
    /// Pending batches (stub - unused).
    pub pending_batches: PendingRocksDBBatches,
}

/// A stub `RocksDB` provider.
///
/// This type exists to allow code to compile when `RocksDB` is not available (either on non-Unix
/// platforms or when the `rocksdb` feature is not enabled). All method calls on `RocksDBProvider`
/// are cfg-guarded in the calling code, so this stub only provides type definitions.
#[derive(Debug, Clone)]
pub struct RocksDBProvider;

impl RocksDBProvider {
    /// Creates a new stub `RocksDB` provider.
    pub fn new(_path: impl AsRef<Path>) -> ProviderResult<Self> {
        Ok(Self)
    }

    /// Creates a new stub `RocksDB` provider builder.
    pub fn builder(path: impl AsRef<Path>) -> RocksDBBuilder {
        RocksDBBuilder::new(path)
    }

    /// Check consistency of `RocksDB` tables (stub implementation).
    ///
    /// Returns `None` since there is no `RocksDB` data to check when the feature is disabled.
    pub const fn check_consistency<Provider>(
        &self,
        _provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }

    /// Returns statistics for all column families in the database (stub implementation).
    ///
    /// Returns an empty vector since there is no `RocksDB` when the feature is disabled.
    pub const fn table_stats(&self) -> Vec<RocksDBTableStats> {
        Vec::new()
    }

    /// Clears all entries from the specified table (stub implementation).
    ///
    /// This is a no-op since there is no `RocksDB` when the feature is disabled.
    pub const fn clear<T>(&self) -> ProviderResult<()> {
        Ok(())
    }

    /// Returns the total size of WAL (Write-Ahead Log) files in bytes (stub implementation).
    ///
    /// Returns 0 since there is no `RocksDB` when the feature is disabled.
    pub const fn wal_size_bytes(&self) -> u64 {
        0
    }

    /// Returns database-level statistics including per-table stats and WAL size (stub
    /// implementation).
    ///
    /// Returns empty stats since there is no `RocksDB` when the feature is disabled.
    pub const fn db_stats(&self) -> RocksDBStats {
        RocksDBStats { tables: Vec::new(), wal_size_bytes: 0 }
    }

    /// Flushes all pending writes to disk (stub implementation).
    ///
    /// This is a no-op since there is no `RocksDB` when the feature is disabled.
    pub const fn flush(&self, _tables: &[&'static str]) -> ProviderResult<()> {
        Ok(())
    }

    /// Creates an iterator over all entries in the specified table (stub implementation).
    ///
    /// Returns an empty iterator since there is no `RocksDB` when the feature is disabled.
    pub const fn iter<T: reth_db_api::table::Table>(&self) -> ProviderResult<RocksDBIter<T>> {
        Ok(RocksDBIter(std::marker::PhantomData))
    }
}

impl DatabaseMetrics for RocksDBProvider {
    fn gauge_metrics(&self) -> Vec<(&'static str, f64, Vec<Label>)> {
        vec![]
    }
}

/// A stub batch writer for `RocksDB`.
#[derive(Debug)]
pub struct RocksDBBatch;

/// A stub builder for `RocksDB`.
#[derive(Debug)]
pub struct RocksDBBuilder;

impl RocksDBBuilder {
    /// Creates a new stub builder.
    pub fn new<P: AsRef<Path>>(_path: P) -> Self {
        Self
    }

    /// Adds a column family for a specific table type (stub implementation).
    pub const fn with_table<T>(self) -> Self {
        self
    }

    /// Registers the default tables used by reth for `RocksDB` storage (stub implementation).
    pub const fn with_default_tables(self) -> Self {
        self
    }

    /// Enables metrics (stub implementation).
    pub const fn with_metrics(self) -> Self {
        self
    }

    /// Enables `RocksDB` internal statistics collection (stub implementation).
    pub const fn with_statistics(self) -> Self {
        self
    }

    /// Sets the log level from `DatabaseArgs` configuration (stub implementation).
    pub const fn with_database_log_level(self, _log_level: Option<LogLevel>) -> Self {
        self
    }

    /// Sets a custom block cache size (stub implementation).
    pub const fn with_block_cache_size(self, _capacity_bytes: usize) -> Self {
        self
    }

    /// Sets read-only mode (stub implementation).
    pub const fn with_read_only(self, _read_only: bool) -> Self {
        self
    }

    /// Build the `RocksDB` provider (stub implementation).
    pub const fn build(self) -> ProviderResult<RocksDBProvider> {
        Ok(RocksDBProvider)
    }
}

/// A stub transaction for `RocksDB`.
#[derive(Debug)]
pub struct RocksTx;

/// A stub raw iterator for `RocksDB`.
#[derive(Debug)]
pub struct RocksDBRawIter;

/// A stub typed iterator for `RocksDB`.
#[derive(Debug)]
pub struct RocksDBIter<T: reth_db_api::table::Table>(std::marker::PhantomData<T>);

impl<T: reth_db_api::table::Table> Iterator for RocksDBIter<T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

/// Outcome of pruning a history shard in `RocksDB` (stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneShardOutcome {
    /// Shard was deleted entirely.
    Deleted,
    /// Shard was updated with filtered block numbers.
    Updated,
    /// Shard was unchanged (no blocks <= `to_block`).
    Unchanged,
}

/// Tracks pruning outcomes for batch operations (stub).
#[derive(Debug, Default, Clone, Copy)]
pub struct PrunedIndices {
    /// Number of shards completely deleted.
    pub deleted: usize,
    /// Number of shards that were updated (filtered but still have entries).
    pub updated: usize,
    /// Number of shards that were unchanged.
    pub unchanged: usize,
}

use alloy_primitives::{Address, B256};
use reth_db_api::{
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey, ShardedKey,
    },
    table::Table,
    tables, BlockNumberList,
};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;

/// Trait unifying `RocksDB` history shard behavior for `AccountsHistory` and `StoragesHistory`.
pub trait HistoryTable: 'static + Send + Sync {
    /// The `RocksDB` table (column family).
    type Table: Table<Value = BlockNumberList>;

    /// The logical entity whose history we store.
    type PartialKey: Clone + core::fmt::Debug + Eq + core::hash::Hash + Send + Sync;

    /// Concrete shard key type used by the table.
    type ShardKey: Clone + core::fmt::Debug + Ord + Send + Sync;

    /// How many block indices fit in a shard for this history type.
    const INDICES_PER_SHARD: u64;

    /// Sentinel used for the "last shard" key.
    const LAST_SHARD_SENTINEL: BlockNumber = u64::MAX;

    /// Create a shard key from partial key + highest block number boundary.
    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey;

    /// Extract partial key from a shard key.
    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey;

    /// Extract highest block boundary from shard key.
    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber;

    /// Return the smallest key for iterating all shards for `partial`.
    fn first_shard_key(partial: Self::PartialKey) -> Self::ShardKey {
        Self::make_shard_key(partial, 0)
    }

    /// Whether `key` belongs to the same partial key.
    fn is_same_partial(key: &Self::ShardKey, partial: &Self::PartialKey) -> bool {
        Self::partial_from_shard_key(key) == *partial
    }
}

/// Marker for `AccountsHistory` table operations.
#[derive(Debug, Clone, Copy)]
pub struct AccountsHistoryTable;

/// Marker for `StoragesHistory` table operations.
#[derive(Debug, Clone, Copy)]
pub struct StoragesHistoryTable;

impl HistoryTable for AccountsHistoryTable {
    type Table = tables::AccountsHistory;
    type PartialKey = Address;
    type ShardKey = ShardedKey<Address>;

    const INDICES_PER_SHARD: u64 = NUM_OF_INDICES_IN_SHARD as u64;

    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey {
        ShardedKey::new(partial, highest)
    }

    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey {
        key.key
    }

    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber {
        key.highest_block_number
    }
}

impl HistoryTable for StoragesHistoryTable {
    type Table = tables::StoragesHistory;
    type PartialKey = (Address, B256);
    type ShardKey = StorageShardedKey;

    const INDICES_PER_SHARD: u64 = NUM_OF_INDICES_IN_SHARD as u64;

    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey {
        StorageShardedKey::new(partial.0, partial.1, highest)
    }

    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey {
        (key.address, key.sharded_key.key)
    }

    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber {
        key.sharded_key.highest_block_number
    }
}

/// Trait for history pipeline metadata (stages, pruning, static files).
pub trait HistorySpec: 'static + Send + Sync {
    /// The underlying history table type.
    type HT: HistoryTable;

    /// Stage ID for indexing this history type.
    const STAGE_ID: StageId;

    /// Prune segment for this history type.
    const PRUNE_SEGMENT: PruneSegment;

    /// Static file segment for the changesets backing this history.
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment;

    /// Whether this history type is stored in `RocksDB` based on settings.
    fn history_in_rocksdb(settings: &StorageSettings) -> bool;

    /// Whether the changesets for this history are in static files.
    fn changesets_in_static_files(settings: &StorageSettings) -> bool;
}

/// `HistorySpec` implementation for account history.
#[derive(Debug, Clone, Copy)]
pub struct AccountHistorySpec;

/// `HistorySpec` implementation for storage history.
#[derive(Debug, Clone, Copy)]
pub struct StorageHistorySpec;

impl HistorySpec for AccountHistorySpec {
    type HT = AccountsHistoryTable;

    const STAGE_ID: StageId = StageId::IndexAccountHistory;
    const PRUNE_SEGMENT: PruneSegment = PruneSegment::AccountHistory;
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment = StaticFileSegment::AccountChangeSets;

    fn history_in_rocksdb(settings: &StorageSettings) -> bool {
        settings.account_history_in_rocksdb
    }

    fn changesets_in_static_files(settings: &StorageSettings) -> bool {
        settings.account_changesets_in_static_files
    }
}

impl HistorySpec for StorageHistorySpec {
    type HT = StoragesHistoryTable;

    const STAGE_ID: StageId = StageId::IndexStorageHistory;
    const PRUNE_SEGMENT: PruneSegment = PruneSegment::StorageHistory;
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment = StaticFileSegment::StorageChangeSets;

    fn history_in_rocksdb(settings: &StorageSettings) -> bool {
        settings.storages_history_in_rocksdb
    }

    fn changesets_in_static_files(settings: &StorageSettings) -> bool {
        settings.storage_changesets_in_static_files
    }
}
