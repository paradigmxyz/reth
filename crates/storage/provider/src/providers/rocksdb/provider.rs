use super::metrics::{RocksDBMetrics, RocksDBOperation};
use reth_db_api::{
    table::{Compress, Decompress, Encode, Table},
    tables, DatabaseError,
};
use reth_storage_errors::{
    db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation, LogLevel},
    provider::{ProviderError, ProviderResult},
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, CompactionPri, DBCompressionType,
    IteratorMode, Options, Transaction, TransactionDB, TransactionDBOptions, TransactionOptions,
    WriteBatchWithTransaction, WriteOptions,
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
    /// Use this method when the batch needs to be held by [`crate::EitherWriter`].
    pub fn batch(&self) -> RocksDBBatch<'_> {
        RocksDBBatch {
            provider: self,
            inner: WriteBatchWithTransaction::<true>::default(),
            buf: Vec::with_capacity(DEFAULT_COMPRESS_BUF_CAPACITY),
        }
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

    /// Writes a batch of operations atomically.
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
}

/// Handle for building a batch of operations atomically.
///
/// Uses `WriteBatchWithTransaction` for atomic writes without full transaction overhead.
/// Unlike [`RocksTx`], this does NOT support read-your-writes. Use for write-only flows
/// where you don't need to read back uncommitted data within the same operation
/// (e.g., history index writes).
#[must_use = "batch must be committed"]
pub struct RocksDBBatch<'a> {
    provider: &'a RocksDBProvider,
    inner: WriteBatchWithTransaction<true>,
    buf: Vec<u8>,
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
        self.provider.0.db.write_opt(self.inner, &WriteOptions::default()).map_err(|e| {
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
}
