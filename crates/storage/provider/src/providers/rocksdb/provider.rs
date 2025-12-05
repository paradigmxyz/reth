use super::metrics::{RocksDBMetrics, RocksDBOperation};
use reth_db_api::{
    table::{Compress, Decompress, Encode, Table},
    DatabaseError,
};
use reth_storage_errors::{
    db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation, LogLevel},
    provider::{ProviderError, ProviderResult},
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, CompactionPri, DBCompressionType, Options,
    WriteBatch, DB,
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
    pub const fn with_database_log_level(mut self, log_level: LogLevel) -> Self {
        self.log_level = convert_log_level(log_level);
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

        let db = DB::open_cf_descriptors(&options, &self.path, cf_descriptors).map_err(|e| {
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
#[derive(Debug)]
struct RocksDBProviderInner {
    /// `RocksDB` database instance.
    db: DB,
    /// Metrics latency & operations.
    metrics: Option<RocksDBMetrics>,
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
        // Note: Using "Batch" as table name for batch operations across multiple tables
        self.execute_with_operation_metric(RocksDBOperation::BatchWrite, "Batch", |this| {
            let mut batch_handle =
                RocksDBBatch { provider: this, inner: WriteBatch::default(), buf: Vec::new() };

            f(&mut batch_handle)?;

            this.0.db.write(batch_handle.inner).map_err(|e| {
                ProviderError::Database(DatabaseError::Commit(DatabaseErrorInfo {
                    message: e.to_string().into(),
                    code: -1,
                }))
            })
        })
    }
}

/// Handle for building a batch of operations atomically.
pub struct RocksDBBatch<'a> {
    provider: &'a RocksDBProvider,
    inner: WriteBatch,
    buf: Vec<u8>,
}

impl<'a> fmt::Debug for RocksDBBatch<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBBatch")
            .field("provider", &self.provider)
            .field("batch", &"<WriteBatch>")
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
    use alloy_primitives::{TxHash, B256};
    use reth_db_api::{table::Table, tables};
    use tempfile::TempDir;

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
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<TestTable>()
            .with_statistics()
            .build()
            .unwrap();

        // Do operations
        for i in 0..10 {
            let value = vec![i as u8];
            provider.put::<TestTable>(i, &value).unwrap();
        }

        // Verify statistics enabled
        let stats = provider.0.db.property_value("rocksdb.stats").unwrap();
        assert!(stats.is_some(), "Statistics should be enabled");
        let stats_str = stats.unwrap();
        assert!(stats_str.contains("DB Stats"));
    }

    #[test]
    fn test_compression_after_flush() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert compressible data
        let value = vec![42u8; 1000];
        for i in 0..100 {
            provider.put::<TestTable>(i, &value).unwrap();
        }

        // Get CF handle and flush it
        let cf = provider.0.db.cf_handle("TestTable").expect("CF should exist");
        provider.0.db.flush_cf(cf).unwrap();

        // Verify data is persisted by reading it back
        for i in 0..100 {
            assert!(provider.get::<TestTable>(i).unwrap().is_some(), "Data should be persisted");
        }
    }
}
