use super::metrics::{RocksDBMetrics, RocksDBOperation};
use crate::providers::HistoryInfo;
use alloy_primitives::{Address, BlockNumber, B256};
use reth_db_api::{
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::{Compress, Decode, Decompress, Encode, Table},
    tables, BlockNumberList, DatabaseError,
};
use reth_storage_errors::{
    db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation, LogLevel},
    provider::{ProviderError, ProviderResult},
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, CompactionPri, DBCompressionType,
    DBRawIteratorWithThreadMode, IteratorMode, Options, Transaction, TransactionDB,
    TransactionDBOptions, TransactionOptions, WriteBatchWithTransaction, WriteOptions,
};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

/// Default cache size for `RocksDB` block cache (128 MB).
const DEFAULT_CACHE_SIZE: usize = 128 << 20;

/// Default block size for `RocksDB` tables (16 KB).
const DEFAULT_BLOCK_SIZE: usize = 16 * 1024;

/// Default max background jobs for `RocksDB` compaction and flushing.
const DEFAULT_MAX_BACKGROUND_JOBS: i32 = 6;

/// Default bytes per sync for `RocksDB` WAL writes (1 MB).
const DEFAULT_BYTES_PER_SYNC: u64 = 1_048_576;

/// Default bloom filter bits per key (~1% false positive rate).
const DEFAULT_BLOOM_FILTER_BITS: f64 = 10.0;

/// Default buffer capacity for compression in batches.
/// 4 KiB matches common block/page sizes and comfortably holds typical history values,
/// reducing the first few reallocations without over-allocating.
const DEFAULT_COMPRESS_BUF_CAPACITY: usize = 4096;

/// Builder for [`RocksDBProvider`].
pub struct RocksDBBuilder {
    path: PathBuf,
    column_families: Vec<String>,
    enable_metrics: bool,
    enable_statistics: bool,
    log_level: rocksdb::LogLevel,
    block_cache: Cache,
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
        // Bloom filter: 10 bits/key = ~1% false positive rate, full filter for better read
        // performance. this setting is good trade off a little bit of memory for better
        // point lookup performance. see https://github.com/facebook/rocksdb/wiki/RocksDB-Bloom-Filter#configuration-basics
        table_options.set_bloom_filter(DEFAULT_BLOOM_FILTER_BITS, false);
        table_options.set_optimize_filters_for_memory(true);
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

    /// Builds the [`RocksDBProvider`].
    pub fn build(self) -> ProviderResult<RocksDBProvider> {
        let options =
            Self::default_options(self.log_level, &self.block_cache, self.enable_statistics);

        let cf_descriptors: Vec<ColumnFamilyDescriptor> = self
            .column_families
            .iter()
            .map(|name| {
                ColumnFamilyDescriptor::new(
                    name.clone(),
                    Self::default_column_family_options(&self.block_cache),
                )
            })
            .collect();

        // Use TransactionDB for MDBX-like transaction semantics (read-your-writes, rollback)
        let txn_db_options = TransactionDBOptions::default();
        let db = TransactionDB::open_cf_descriptors(
            &options,
            &txn_db_options,
            &self.path,
            cf_descriptors,
        )
        .map_err(|e| {
            ProviderError::Database(DatabaseError::Open(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })?;

        let metrics = self.enable_metrics.then(RocksDBMetrics::default);

        Ok(RocksDBProvider(Arc::new(RocksDBProviderInner { db, metrics })))
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
struct RocksDBProviderInner {
    /// `RocksDB` database instance with transaction support.
    db: TransactionDB,
    /// Metrics latency & operations.
    metrics: Option<RocksDBMetrics>,
}

impl fmt::Debug for RocksDBProviderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBProviderInner")
            .field("db", &"<TransactionDB>")
            .field("metrics", &self.metrics)
            .finish()
    }
}

impl Clone for RocksDBProvider {
    fn clone(&self) -> Self {
        Self(self.0.clone())
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

    /// Creates a new transaction with MDBX-like semantics (read-your-writes, rollback).
    pub fn tx(&self) -> RocksTx<'_> {
        let write_options = WriteOptions::default();
        let txn_options = TransactionOptions::default();
        let inner = self.0.db.transaction_opt(&write_options, &txn_options);
        RocksTx { inner, provider: self }
    }

    /// Creates a new batch for atomic writes.
    ///
    /// Use [`Self::write_batch`] for closure-based atomic writes.
    /// Use this method when the batch needs to be held by [`crate::EitherWriter`]
    /// or stored in `DatabaseProvider`.
    pub fn batch(&self) -> RocksDBBatch {
        RocksDBBatch::new(self.clone())
    }

    /// Gets the column family handle for a table.
    fn get_cf_handle<T: Table>(&self) -> Result<&rocksdb::ColumnFamily, DatabaseError> {
        self.0
            .db
            .cf_handle(T::NAME)
            .ok_or_else(|| DatabaseError::Other(format!("Column family '{}' not found", T::NAME)))
    }

    /// Executes a function and records metrics with the given operation and table name.
    fn execute_with_operation_metric<T>(
        &self,
        operation: RocksDBOperation,
        table: &'static str,
        f: impl FnOnce(&Self) -> T,
    ) -> T {
        let start = self.0.metrics.as_ref().map(|_| Instant::now());
        let res = f(self);

        if let (Some(start), Some(metrics)) = (start, &self.0.metrics) {
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
            let result =
                this.0.db.get_cf(this.get_cf_handle::<T>()?, key.as_ref()).map_err(|e| {
                    ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                        message: e.to_string().into(),
                        code: -1,
                    }))
                })?;

            Ok(result.and_then(|value| T::Value::decompress(&value).ok()))
        })
    }

    /// Puts upsert a value into the specified table with the given key.
    pub fn put<T: Table>(&self, key: T::Key, value: &T::Value) -> ProviderResult<()> {
        let encoded_key = key.encode();
        self.put_encoded::<T>(&encoded_key, value)
    }

    /// Puts a value into the specified table using pre-encoded key.
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

            this.0.db.put_cf(this.get_cf_handle::<T>()?, key, value_bytes).map_err(|e| {
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
    pub fn delete<T: Table>(&self, key: T::Key) -> ProviderResult<()> {
        self.execute_with_operation_metric(RocksDBOperation::Delete, T::NAME, |this| {
            this.0.db.delete_cf(this.get_cf_handle::<T>()?, key.encode().as_ref()).map_err(|e| {
                ProviderError::Database(DatabaseError::Delete(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))
            })
        })
    }

    /// Gets the first (smallest key) entry from the specified table.
    pub fn first<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.execute_with_operation_metric(RocksDBOperation::Get, T::NAME, |this| {
            let cf = this.get_cf_handle::<T>()?;
            let mut iter = this.0.db.iterator_cf(cf, IteratorMode::Start);

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

    /// Gets the last (largest key) entry from the specified table.
    pub fn last<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.execute_with_operation_metric(RocksDBOperation::Get, T::NAME, |this| {
            let cf = this.get_cf_handle::<T>()?;
            let mut iter = this.0.db.iterator_cf(cf, IteratorMode::End);

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

    /// Creates an iterator over all entries in the specified table.
    ///
    /// Returns decoded `(Key, Value)` pairs in key order.
    pub fn iter<T: Table>(&self) -> ProviderResult<RocksDBIter<'_, T>> {
        let cf = self.get_cf_handle::<T>()?;
        let iter = self.0.db.iterator_cf(cf, IteratorMode::Start);
        Ok(RocksDBIter { inner: iter, _marker: std::marker::PhantomData })
    }

    /// Writes a batch of operations atomically.
    pub fn write_batch<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(&mut RocksDBBatch) -> ProviderResult<()>,
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
    pub fn commit_batch(&self, batch: WriteBatchWithTransaction<true>) -> ProviderResult<()> {
        self.0.db.write_opt(batch, &WriteOptions::default()).map_err(|e| {
            ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }
}

/// Handle for building a batch of operations atomically.
///
/// Uses `WriteBatchWithTransaction` for atomic write-only batching.
/// This does NOT support read-your-writes; use [`RocksTx`] (or an optimistic transaction)
/// if you need to read within the same logical operation.
///
/// Note: `WriteBatch` operations are applied in order. If the same key is updated multiple times,
/// the last update wins. Ref: <https://github.com/facebook/rocksdb/wiki/Basic-Operations#atomic-updates>
///
/// This type owns a `RocksDBProvider` to allow storing it in `DatabaseProvider`
/// without lifetime issues.
#[must_use = "batch must be committed"]
pub struct RocksDBBatch {
    provider: RocksDBProvider,
    inner: WriteBatchWithTransaction<true>,
    buf: Vec<u8>,
}

impl fmt::Debug for RocksDBBatch {
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

impl RocksDBBatch {
    /// Creates a new batch for the given provider.
    pub(crate) fn new(provider: RocksDBProvider) -> Self {
        Self {
            provider,
            inner: WriteBatchWithTransaction::<true>::default(),
            buf: Vec::with_capacity(DEFAULT_COMPRESS_BUF_CAPACITY),
        }
    }
    /// Puts a value into the batch.
    pub fn put<T: Table>(&mut self, key: T::Key, value: &T::Value) -> ProviderResult<()> {
        let encoded_key = key.encode();
        self.put_encoded::<T>(&encoded_key, value)
    }

    /// Puts a value into the batch using pre-encoded key.
    pub fn put_encoded<T: Table>(
        &mut self,
        key: &<T::Key as Encode>::Encoded,
        value: &T::Value,
    ) -> ProviderResult<()> {
        let value_bytes = compress_to_buf_or_ref!(self.buf, value).unwrap_or(&self.buf);
        self.inner.put_cf(self.provider.get_cf_handle::<T>()?, key, value_bytes);
        Ok(())
    }

    /// Deletes a value from the batch.
    pub fn delete<T: Table>(&mut self, key: T::Key) -> ProviderResult<()> {
        self.inner.delete_cf(self.provider.get_cf_handle::<T>()?, key.encode().as_ref());
        Ok(())
    }

    /// Commits the batch to the database.
    ///
    /// This consumes the batch and writes all operations atomically to `RocksDB`.
    pub fn commit(self) -> ProviderResult<()> {
        self.provider.commit_batch(self.inner)
    }

    /// Returns the number of write operations (puts + deletes) queued in this batch.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the batch contains no operations.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Consumes the batch and returns the underlying `WriteBatchWithTransaction`.
    ///
    /// This is used to defer commits to the provider level.
    pub fn into_inner(self) -> WriteBatchWithTransaction<true> {
        self.inner
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
    inner: Transaction<'db, TransactionDB>,
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
    pub fn commit(self) -> ProviderResult<()> {
        self.inner.commit().map_err(|e| {
            ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }

    /// Rolls back the transaction, discarding all changes.
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
        // Raw iterator is needed for the backward check via `prev()`.
        let mut iter = self.raw_iterator_cf::<T>()?;
        iter.seek::<&[u8]>(encoded_key);
        Self::raw_iter_status_ok(&iter)?;

        if !iter.valid() {
            return Ok(if lowest_available_block_number.is_some() {
                HistoryInfo::MaybeInPlainState
            } else {
                HistoryInfo::NotYetWritten
            })
        }

        let key_bytes =
            iter.key().ok_or(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                message: "missing key".into(),
                code: -1,
            })))?;
        let value_bytes = iter.value().ok_or(ProviderError::Database(DatabaseError::Read(
            DatabaseErrorInfo { message: "missing value".into(), code: -1 },
        )))?;

        if !key_matches(key_bytes).map_err(ProviderError::Database)? {
            return Ok(if lowest_available_block_number.is_some() {
                HistoryInfo::MaybeInPlainState
            } else {
                HistoryInfo::NotYetWritten
            })
        }

        let chunk = T::Value::decompress(value_bytes)
            .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;

        // Get rank of first entry <= block_number, then adjust to get entry strictly before.
        let mut rank = chunk.rank(block_number);
        if rank.checked_sub(1).and_then(|r| chunk.select(r)) == Some(block_number) {
            rank -= 1;
        }
        let found_block = chunk.select(rank);

        // Lazy check for previous shard - only called when needed.
        // If we can step to a previous shard for this same key, history already exists,
        // so the target block is not before the first write.
        let is_before_first_write = if rank == 0 && found_block != Some(block_number) {
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

    /// Creates a raw iterator for bidirectional traversal of the specified table.
    ///
    /// Unlike [`Self::iter_from`], raw iterators support `prev()` for backward navigation.
    /// Use this for history lookups where checking the previous entry is needed.
    fn raw_iterator_cf<T: Table>(
        &self,
    ) -> ProviderResult<DBRawIteratorWithThreadMode<'_, Transaction<'_, TransactionDB>>> {
        let cf = self.provider.get_cf_handle::<T>()?;
        Ok(self.inner.raw_iterator_cf(cf))
    }

    /// Raw iterators surface I/O errors via `status()`, not through `key()`/`value()`.
    /// Call this after `seek`/`next`/`prev` before interpreting `valid()`.
    fn raw_iter_status_ok(
        iter: &DBRawIteratorWithThreadMode<'_, Transaction<'_, TransactionDB>>,
    ) -> ProviderResult<()> {
        iter.status().map_err(|e| {
            ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                message: e.to_string().into(),
                code: -1,
            }))
        })
    }
}

/// Iterator over a `RocksDB` table (non-transactional).
///
/// Yields decoded `(Key, Value)` pairs in key order.
pub struct RocksDBIter<'db, T: Table> {
    inner: rocksdb::DBIteratorWithThreadMode<'db, TransactionDB>,
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
        let (key_bytes, value_bytes) = match self.inner.next()? {
            Ok(kv) => kv,
            Err(e) => {
                return Some(Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))))
            }
        };

        // Decode key
        let key = match <T::Key as reth_db_api::table::Decode>::decode(&key_bytes) {
            Ok(k) => k,
            Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
        };

        // Decompress value
        let value = match T::Value::decompress(&value_bytes) {
            Ok(v) => v,
            Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
        };

        Some(Ok((key, value)))
    }
}

/// Iterator over a `RocksDB` table within a transaction.
///
/// Yields decoded `(Key, Value)` pairs. Sees uncommitted writes.
pub struct RocksTxIter<'tx, T: Table> {
    inner: rocksdb::DBIteratorWithThreadMode<'tx, Transaction<'tx, TransactionDB>>,
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
        let (key_bytes, value_bytes) = match self.inner.next()? {
            Ok(kv) => kv,
            Err(e) => {
                return Some(Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))))
            }
        };

        // Decode key
        let key = match <T::Key as reth_db_api::table::Decode>::decode(&key_bytes) {
            Ok(k) => k,
            Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
        };

        // Decompress value
        let value = match T::Value::decompress(&value_bytes) {
            Ok(v) => v,
            Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
        };

        Some(Ok((key, value)))
    }
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
    use alloy_primitives::{Address, TxHash, B256};
    use reth_db_api::{
        models::{sharded_key::ShardedKey, storage_sharded_key::StorageShardedKey, IntegerList},
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

        // Do operations - data should be immediately readable with TransactionDB
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

        // Insert data - TransactionDB writes are immediately visible
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

    #[test]
    fn test_account_history_info_single_shard() {
        use crate::providers::HistoryInfo;

        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create a single shard with blocks [100, 200, 300] and highest_block = u64::MAX
        // This is the "last shard" invariant
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = ShardedKey::new(address, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query for block 150: should find block 200 in changeset
        let result = tx.account_history_info(address, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(200));

        // Query for block 50: should return NotYetWritten (before first entry, no prev shard)
        let result = tx.account_history_info(address, 50, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        // Query for block 300: should return InChangeset(300) - exact match means look at
        // changeset at that block for the previous value
        let result = tx.account_history_info(address, 300, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(300));

        // Query for block 500: should return InPlainState (after last entry in last shard)
        let result = tx.account_history_info(address, 500, None).unwrap();
        assert_eq!(result, HistoryInfo::InPlainState);

        tx.rollback().unwrap();
    }

    #[test]
    fn test_account_history_info_multiple_shards() {
        use crate::providers::HistoryInfo;

        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Create two shards: first shard ends at block 500, second is the last shard
        let chunk1 = IntegerList::new([100, 200, 300, 400, 500]).unwrap();
        let shard_key1 = ShardedKey::new(address, 500);
        provider.put::<tables::AccountsHistory>(shard_key1, &chunk1).unwrap();

        let chunk2 = IntegerList::new([600, 700, 800]).unwrap();
        let shard_key2 = ShardedKey::new(address, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key2, &chunk2).unwrap();

        let tx = provider.tx();

        // Query for block 50: should return NotYetWritten (before first shard, no prev)
        let result = tx.account_history_info(address, 50, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        // Query for block 150: should find block 200 in first shard's changeset
        let result = tx.account_history_info(address, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(200));

        // Query for block 550: should find block 600 in second shard's changeset
        // prev() should detect first shard exists
        let result = tx.account_history_info(address, 550, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(600));

        // Query for block 900: should return InPlainState (after last entry in last shard)
        let result = tx.account_history_info(address, 900, None).unwrap();
        assert_eq!(result, HistoryInfo::InPlainState);

        tx.rollback().unwrap();
    }

    #[test]
    fn test_account_history_info_no_history() {
        use crate::providers::HistoryInfo;

        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address1 = Address::from([0x42; 20]);
        let address2 = Address::from([0x43; 20]);

        // Create history only for address1 to test cross-address isolation
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = ShardedKey::new(address1, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query for address2 (no history) should return NotYetWritten
        let result = tx.account_history_info(address2, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        // With lowest_available_block_number set, should return MaybeInPlainState
        let result = tx.account_history_info(address2, 150, Some(50)).unwrap();
        assert_eq!(result, HistoryInfo::MaybeInPlainState);

        // Query for address1 should find the history (verifies isolation works both ways)
        let result = tx.account_history_info(address1, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(200));

        tx.rollback().unwrap();
    }

    #[test]
    fn test_account_history_info_pruned_before_first_entry() {
        use crate::providers::HistoryInfo;

        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);

        // Single shard with entries starting at 100.
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = ShardedKey::new(address, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query before first entry with pruning boundary should return InChangeset at first entry.
        let result = tx.account_history_info(address, 50, Some(10)).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(100));

        tx.rollback().unwrap();
    }

    #[test]
    fn test_storage_history_info() {
        use crate::providers::HistoryInfo;

        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let storage_key1 = B256::from([0x11; 32]);
        let storage_key2 = B256::from([0x22; 32]);

        // Create two shards for storage_key1 to test multi-shard behavior
        let chunk1 = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key1 = StorageShardedKey::new(address, storage_key1, 300);
        provider.put::<tables::StoragesHistory>(shard_key1, &chunk1).unwrap();

        let chunk2 = IntegerList::new([400, 500, 600]).unwrap();
        let shard_key2 = StorageShardedKey::new(address, storage_key1, u64::MAX);
        provider.put::<tables::StoragesHistory>(shard_key2, &chunk2).unwrap();

        let tx = provider.tx();

        // Query for block 150: should find block 200 in first shard
        let result = tx.storage_history_info(address, storage_key1, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(200));

        // Query for block 50: should return NotYetWritten (before first entry)
        let result = tx.storage_history_info(address, storage_key1, 50, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        // Query for block 350: should find block 400 in second shard
        // prev() should detect first shard exists (tests has_prev)
        let result = tx.storage_history_info(address, storage_key1, 350, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(400));

        // Query for storage_key2 (no history) should return NotYetWritten
        // Tests cross-slot isolation
        let result = tx.storage_history_info(address, storage_key2, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        tx.rollback().unwrap();
    }
}
