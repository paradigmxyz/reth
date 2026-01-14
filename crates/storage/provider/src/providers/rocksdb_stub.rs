//! Stub implementation of `RocksDB` provider.
//!
//! This module provides placeholder types that allow the code to compile when `RocksDB` is not
//! available (either on non-Unix platforms or when the `rocksdb` feature is not enabled).
//! Operations will produce errors if actually attempted.

use alloy_primitives::BlockNumber;
use parking_lot::Mutex;
use reth_db_api::{
    models::StorageSettings,
    table::{Encode, Table},
};
use reth_prune_types::PruneMode;
use reth_storage_errors::{
    db::LogLevel,
    provider::{ProviderError::UnsupportedProvider, ProviderResult},
};
use std::{path::Path, sync::Arc};

/// Pending RocksDB batches type alias (stub - uses unit type).
pub type PendingRocksDBBatches = Arc<Mutex<Vec<()>>>;

/// Context for `RocksDB` block writes (stub).
#[derive(Debug, Clone)]
pub struct RocksDBWriteCtx {
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

    /// Gets the first key-value pair from a table (stub implementation).
    pub const fn first<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        Ok(None)
    }

    /// Gets the last key-value pair from a table (stub implementation).
    pub const fn last<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        Ok(None)
    }

    /// Creates an iterator for the specified table (stub implementation).
    ///
    /// Returns an empty iterator. This is consistent with `first()` and `last()` returning
    /// `Ok(None)` - the stub behaves as if the database is empty rather than unavailable.
    pub const fn iter<T: Table>(&self) -> ProviderResult<RocksDBIter<'_, T>> {
        Ok(RocksDBIter { _marker: std::marker::PhantomData })
    }

    /// Check consistency of `RocksDB` tables (stub implementation).
    ///
    /// Returns `None` since there is no `RocksDB` data to check when the feature is disabled.
    pub const fn check_consistency<Provider>(
        &self,
        _provider: &Provider,
    ) -> ProviderResult<Option<alloy_primitives::BlockNumber>> {
        Ok(None)
    }

    /// Writes all `RocksDB` data for multiple blocks (stub implementation).
    ///
    /// No-op since `RocksDB` is not available on this platform.
    pub fn write_blocks_data<N>(
        &self,
        _blocks: &[reth_chain_state::ExecutedBlock<N>],
        _tx_nums: &[alloy_primitives::TxNumber],
        _ctx: RocksDBWriteCtx,
    ) -> ProviderResult<()>
    where
        N: reth_node_types::NodePrimitives,
    {
        Ok(())
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
}

/// A stub iterator for `RocksDB` (non-transactional).
#[derive(Debug)]
pub struct RocksDBIter<'a, T> {
    _marker: std::marker::PhantomData<(&'a (), T)>,
}

impl<T: Table> Iterator for RocksDBIter<'_, T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        None
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

impl<T: Table> Iterator for RocksTxIter<'_, T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}
