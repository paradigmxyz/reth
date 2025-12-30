//! Stub implementation of `RocksDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when `RocksDB` is not
//! available (either on non-Unix platforms or when the `rocksdb` feature is not enabled).
//! Operations will produce errors if actually attempted.

use reth_db_api::table::{Encode, Table};
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
    pub fn new(_path: impl AsRef<Path>) -> ProviderResult<Self> {
        Ok(Self)
    }

    /// Creates a new stub `RocksDB` provider builder.
    pub fn builder(path: impl AsRef<Path>) -> RocksDBBuilder {
        RocksDBBuilder::new(path)
    }

    /// Get a value from `RocksDB` (stub implementation).
    pub fn get<T: Table>(&self, _key: T::Key) -> ProviderResult<Option<T::Value>> {
        Err(UnsupportedProvider)
    }

    /// Get a value from `RocksDB` using pre-encoded key (stub implementation).
    pub const fn get_encoded<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
    ) -> ProviderResult<Option<T::Value>> {
        Err(UnsupportedProvider)
    }

    /// Put a value into `RocksDB` (stub implementation).
    pub fn put<T: Table>(&self, _key: T::Key, _value: &T::Value) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Put a value into `RocksDB` using pre-encoded key (stub implementation).
    pub const fn put_encoded<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
        _value: &T::Value,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Delete a value from `RocksDB` (stub implementation).
    pub fn delete<T: Table>(&self, _key: T::Key) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Write a batch of operations (stub implementation).
    pub fn write_batch<F>(&self, _f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut RocksDBBatch) -> ProviderResult<()>,
    {
        Err(UnsupportedProvider)
    }

    /// Creates a new transaction (stub implementation).
    pub const fn tx(&self) -> RocksTx {
        RocksTx
    }

    /// Creates a new batch for atomic writes (stub implementation).
    pub const fn batch(&self) -> RocksDBBatch {
        RocksDBBatch
    }
}

/// A stub batch writer for `RocksDB` on non-Unix platforms.
#[derive(Debug)]
pub struct RocksDBBatch;

impl RocksDBBatch {
    /// Puts a value into the batch (stub implementation).
    pub fn put<T: Table>(&self, _key: T::Key, _value: &T::Value) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Puts a value into the batch using pre-encoded key (stub implementation).
    pub const fn put_encoded<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
        _value: &T::Value,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Deletes a value from the batch (stub implementation).
    pub fn delete<T: Table>(&self, _key: T::Key) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Commits the batch (stub implementation).
    pub const fn commit(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Returns the number of operations in this batch (stub implementation).
    pub const fn len(&self) -> usize {
        0
    }

    /// Returns `true` if the batch is empty (stub implementation).
    pub const fn is_empty(&self) -> bool {
        true
    }

    /// Consumes the batch and returns the inner batch (stub returns nothing).
    pub fn into_inner(self) {}
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

    /// Build the `RocksDB` provider (stub implementation).
    pub const fn build(self) -> ProviderResult<RocksDBProvider> {
        Ok(RocksDBProvider)
    }
}

/// A stub transaction for `RocksDB`.
#[derive(Debug)]
pub struct RocksTx;

impl RocksTx {
    /// Gets a value from the specified table (stub implementation).
    pub fn get<T: Table>(&self, _key: T::Key) -> ProviderResult<Option<T::Value>> {
        Err(UnsupportedProvider)
    }

    /// Gets a value using pre-encoded key (stub implementation).
    pub const fn get_encoded<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
    ) -> ProviderResult<Option<T::Value>> {
        Err(UnsupportedProvider)
    }

    /// Puts a value into the specified table (stub implementation).
    pub fn put<T: Table>(&self, _key: T::Key, _value: &T::Value) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Puts a value using pre-encoded key (stub implementation).
    pub const fn put_encoded<T: Table>(
        &self,
        _key: &<T::Key as Encode>::Encoded,
        _value: &T::Value,
    ) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Deletes a value from the specified table (stub implementation).
    pub fn delete<T: Table>(&self, _key: T::Key) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Creates an iterator for the specified table (stub implementation).
    pub const fn iter<T: Table>(&self) -> ProviderResult<RocksTxIter<'_, T>> {
        Err(UnsupportedProvider)
    }

    /// Creates an iterator starting from the given key (stub implementation).
    pub fn iter_from<T: Table>(&self, _key: T::Key) -> ProviderResult<RocksTxIter<'_, T>> {
        Err(UnsupportedProvider)
    }

    /// Commits the transaction (stub implementation).
    pub const fn commit(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }

    /// Rolls back the transaction (stub implementation).
    pub const fn rollback(self) -> ProviderResult<()> {
        Err(UnsupportedProvider)
    }
}

/// A stub iterator for `RocksDB` transactions.
#[derive(Debug)]
pub struct RocksTxIter<'a, T> {
    _marker: std::marker::PhantomData<(&'a (), T)>,
}
