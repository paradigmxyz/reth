//! Stub implementation of `RocksDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when `RocksDB` is not
//! available (either on non-Unix platforms or when the `rocksdb` feature is not enabled).
//! All method calls are cfg-guarded in the calling code, so only type definitions are needed here.

use alloy_primitives::BlockNumber;
use parking_lot::Mutex;
use reth_db_api::models::StorageSettings;
use reth_prune_types::PruneMode;
use reth_storage_errors::provider::ProviderResult;
use std::{path::Path, sync::Arc};

/// Pending RocksDB batches type alias (stub - uses unit type).
pub type PendingRocksDBBatches = Arc<Mutex<Vec<()>>>;

/// Context for RocksDB block writes (stub).
#[derive(Debug, Clone)]
pub struct RocksDBWriteCtx {
    /// The first block number being written.
    pub first_block_number: BlockNumber,
    /// The prune mode for transaction lookup, if any.
    pub prune_tx_lookup: Option<PruneMode>,
    /// Storage settings determining what goes to RocksDB.
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
    pub const fn with_database_log_level(
        self,
        _log_level: Option<reth_storage_errors::db::LogLevel>,
    ) -> Self {
        self
    }

    /// Sets a custom block cache size (stub implementation).
    pub const fn with_block_cache_size(self, _capacity_bytes: usize) -> Self {
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
