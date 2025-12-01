//! Stub implementation of `RocksDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when `RocksDB` is not
//! available (either on non-Unix platforms or when the `rocksdb` feature is not enabled).
//! Operations will produce errors if actually attempted.

use reth_db_api::table::{Decompress, Table};
use reth_storage_errors::{
    db::LogLevel,
    provider::{ProviderError::UnsupportedProvider, ProviderResult},
};
use std::path::Path;

/// A stub `RocksDB` provider.
///
/// This type exists to allow code to compile when `RocksDB` is not available (either on non-Unix
/// platforms or when the `rocksdb` feature is not enabled). When using this stub, the
/// `transaction_hash_numbers_in_rocksdb` flag should be set to `false` to ensure all operations
/// route to MDBX instead.
#[derive(Debug, Clone)]
pub struct RocksDBProvider;

impl RocksDBProvider {
    /// Creates a new stub `RocksDB` provider.
    ///
    /// On non-Unix platforms, this returns an error indicating `RocksDB` is not supported.
    pub fn new<P: AsRef<Path>>(_path: P) -> ProviderResult<Self> {
        Ok(Self)
    }

    /// Creates a new stub `RocksDB` provider builder.
    pub fn builder<P: AsRef<Path>>(path: P) -> RocksDBBuilder {
        RocksDBBuilder::new(path)
    }

    /// Get a value from `RocksDB` (stub implementation).
    pub fn get<T>(&self, _key: T::Key) -> ProviderResult<Option<T::Value>>
    where
        T: Table,
        T::Value: Decompress,
    {
        Err(UnsupportedProvider)
    }

    /// Put a value into `RocksDB` (stub implementation).
    pub fn put<T>(&self, _key: T::Key, _value: T::Value) -> ProviderResult<()>
    where
        T: Table,
    {
        Err(UnsupportedProvider)
    }

    /// Delete a value from `RocksDB` (stub implementation).
    pub fn delete<T>(&self, _key: T::Key) -> ProviderResult<()>
    where
        T: Table,
    {
        Err(UnsupportedProvider)
    }

    /// Write a batch of operations (stub implementation).
    pub fn write_batch<F>(&self, _f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut RocksDBBatch) -> ProviderResult<()>,
    {
        Err(UnsupportedProvider)
    }
}

/// A stub batch writer for `RocksDB` on non-Unix platforms.
#[derive(Debug)]
pub struct RocksDBBatch;

impl RocksDBBatch {
    /// Put a value into the batch (stub implementation).
    pub fn put<T>(&self, _key: T::Key, _value: T::Value) -> ProviderResult<()>
    where
        T: Table,
    {
        Err(UnsupportedProvider)
    }
}

/// A stub builder for `RocksDB` on non-Unix platforms.
#[derive(Debug)]
pub struct RocksDBBuilder;

impl RocksDBBuilder {
    /// Creates a new stub builder.
    pub fn new<P: AsRef<Path>>(_path: P) -> Self {
        Self
    }

    /// Adds a column family for a specific table type (stub implementation).
    pub const fn with_table<T: Table>(self) -> Self {
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

    /// Opens the database in read-only mode (stub implementation).
    pub const fn read_only(self) -> Self {
        self
    }

    /// Build the `RocksDB` provider (stub implementation).
    pub const fn build(self) -> ProviderResult<RocksDBProvider> {
        Ok(RocksDBProvider)
    }
}
