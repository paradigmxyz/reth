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

    /// Flushes all column family memtables to SST files (stub implementation).
    ///
    /// This is a no-op since there is no `RocksDB` when the feature is disabled.
    pub const fn flush(&self) -> ProviderResult<()> {
        Ok(())
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
