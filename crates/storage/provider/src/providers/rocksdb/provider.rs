use super::metrics::{RocksDBMetrics, RocksDBOperation};
use crate::providers::{needs_prev_shard_check, HistoryInfo};
use alloy_consensus::transaction::TxHashRef;
use alloy_primitives::{Address, BlockNumber, TxNumber, B256};
use itertools::Itertools;
use parking_lot::Mutex;
use reth_chain_state::ExecutedBlock;
use reth_db_api::{
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey, ShardedKey,
        StorageSettings,
    },
    table::{Compress, Decode, Decompress, Encode, Table},
    tables, BlockNumberList, DatabaseError,
};
use reth_primitives_traits::BlockBody as _;
use reth_prune_types::PruneMode;
use reth_storage_errors::{
    db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation, LogLevel},
    provider::{ProviderError, ProviderResult},
};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, CompactionPri, DBCompressionType,
    DBRawIteratorWithThreadMode, IteratorMode, OptimisticTransactionDB,
    OptimisticTransactionOptions, Options, Transaction, WriteBatchWithTransaction, WriteOptions,
};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

/// Pending `RocksDB` batches type alias.
pub type PendingRocksDBBatches = Arc<Mutex<Vec<WriteBatchWithTransaction<true>>>>;

/// Context for `RocksDB` block writes.
#[derive(Clone)]
pub struct RocksDBWriteCtx {
    /// The first block number being written.
    pub first_block_number: BlockNumber,
    /// The prune mode for transaction lookup, if any.
    pub prune_tx_lookup: Option<PruneMode>,
    /// Storage settings determining what goes to RocksDB.
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

        // Use OptimisticTransactionDB for MDBX-like transaction semantics (read-your-writes,
        // rollback) OptimisticTransactionDB uses optimistic concurrency control (conflict
        // detection at commit) and is backed by DBCommon, giving us access to
        // cancel_all_background_work for clean shutdown.
        let db = OptimisticTransactionDB::open_cf_descriptors(&options, &self.path, cf_descriptors)
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
///
/// Returns a `&[u8]` directly - either the uncompressable reference or the compressed buffer.
macro_rules! compress_to_buf_or_ref {
    ($buf:expr, $value:expr) => {{
        if let Some(value) = $value.uncompressable_ref() {
            value
        } else {
            $buf.clear();
            $value.compress_to_buf(&mut $buf);
            &$buf[..]
        }
    }};
}

/// `RocksDB` provider for auxiliary storage layer beside main database MDBX.
#[derive(Debug)]
pub struct RocksDBProvider(Arc<RocksDBProviderInner>);

/// Inner state for `RocksDB` provider.
struct RocksDBProviderInner {
    /// `RocksDB` database instance with optimistic transaction support.
    db: OptimisticTransactionDB,
    /// Metrics latency & operations.
    metrics: Option<RocksDBMetrics>,
}

impl fmt::Debug for RocksDBProviderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBProviderInner")
            .field("db", &"<OptimisticTransactionDB>")
            .field("metrics", &self.metrics)
            .finish()
    }
}

impl Drop for RocksDBProviderInner {
    fn drop(&mut self) {
        // Cancel all background work (compaction, flush) before dropping.
        // This prevents pthread lock errors during shutdown.
        self.db.cancel_all_background_work(true);
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
    ///
    /// Note: With `OptimisticTransactionDB`, commits may fail if there are conflicts.
    /// Conflict detection happens at commit time, not at write time.
    pub fn tx(&self) -> RocksTx<'_> {
        let write_options = WriteOptions::default();
        let txn_options = OptimisticTransactionOptions::default();
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
            let result = this
                .0
                .db
                .get_cf(this.get_cf_handle::<T>()?, key.as_ref())
                .map_err(|e| this.read_error(e))?;

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
            // For thread-safe operation, we allocate buf here each time.
            // To avoid per-call allocation, use the write_batch API instead.
            let mut buf = Vec::new();
            let value_bytes = compress_to_buf_or_ref!(buf, value);

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
            this.0
                .db
                .delete_cf(this.get_cf_handle::<T>()?, key.encode().as_ref())
                .map_err(|e| this.delete_error(e))
        })
    }

    /// Clears all entries from the specified table.
    ///
    /// This iterates through all keys and deletes them one by one.
    /// For large tables, consider using a batch operation instead.
    pub fn clear<T: Table>(&self) -> ProviderResult<()> {
        let cf = self.get_cf_handle::<T>()?;

        // Collect all keys first to avoid iterator invalidation during deletion
        let keys: Vec<_> = self
            .0
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .filter_map(|result| result.ok().map(|(k, _)| k.to_vec()))
            .collect();

        // Delete each key
        for key in keys {
            self.0.db.delete_cf(cf, &key).map_err(|e| self.delete_error(e))?;
        }

        Ok(())
    }

    /// Gets the first (smallest key) entry from the specified table.
    pub fn first<T: Table>(&self) -> ProviderResult<Option<(T::Key, T::Value)>> {
        self.execute_with_operation_metric(RocksDBOperation::Get, T::NAME, |this| {
            let cf = this.get_cf_handle::<T>()?;
            let mut iter = this.0.db.iterator_cf(cf, IteratorMode::Start);

            match iter.next() {
                Some(Ok((key_bytes, value_bytes))) => this.decode_kv::<T>(&key_bytes, &value_bytes),
                Some(Err(e)) => Err(this.read_error(e)),
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
                Some(Ok((key_bytes, value_bytes))) => this.decode_kv::<T>(&key_bytes, &value_bytes),
                Some(Err(e)) => Err(this.read_error(e)),
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

    /// Creates an iterator starting from the given key (inclusive) in reverse order.
    ///
    /// Returns decoded `(Key, Value)` pairs in reverse key order, starting from `key`.
    pub fn iter_from_reverse<T: Table>(
        &self,
        key: T::Key,
    ) -> ProviderResult<RocksDBIterReverse<'_, T>> {
        let cf = self.get_cf_handle::<T>()?;
        let encoded_key = key.encode();
        let iter = self
            .0
            .db
            .iterator_cf(cf, IteratorMode::From(encoded_key.as_ref(), rocksdb::Direction::Reverse));
        Ok(RocksDBIterReverse { inner: iter, _marker: std::marker::PhantomData })
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

    /// Writes all RocksDB data for multiple blocks in a single batch.
    ///
    /// This handles transaction hash numbers, account history, and storage history based on
    /// the provided storage settings. Returns the raw batch to be committed later, or `None` if no
    /// RocksDB writes are needed.
    pub fn write_blocks_data<N: reth_node_types::NodePrimitives>(
        &self,
        blocks: &[ExecutedBlock<N>],
        tx_nums: &[TxNumber],
        ctx: RocksDBWriteCtx,
    ) -> ProviderResult<Option<WriteBatchWithTransaction<true>>> {
        if !ctx.storage_settings.any_in_rocksdb() {
            return Ok(None);
        }

        let mut batch = self.batch();

        // Write transaction hash numbers
        if ctx.storage_settings.transaction_hash_numbers_in_rocksdb &&
            ctx.prune_tx_lookup.is_none_or(|m| !m.is_full())
        {
            for (block, &first_tx_num) in blocks.iter().zip(tx_nums) {
                let body = block.recovered_block().body();
                let mut tx_num = first_tx_num;
                for transaction in body.transactions_iter() {
                    batch.put::<tables::TransactionHashNumbers>(*transaction.tx_hash(), &tx_num)?;
                    tx_num += 1;
                }
            }
        }

        // Collect and write account history with proper shard management
        if ctx.storage_settings.account_history_in_rocksdb {
            let mut account_history: BTreeMap<Address, Vec<u64>> = BTreeMap::new();
            for (block_idx, block) in blocks.iter().enumerate() {
                let block_number = ctx.first_block_number + block_idx as u64;
                let bundle = &block.execution_outcome().bundle;
                for (&address, _) in bundle.state() {
                    account_history.entry(address).or_default().push(block_number);
                }
            }

            // Write account history using proper shard append logic
            for (address, indices) in account_history {
                batch.append_account_history_shard(address, indices)?;
            }
        }

        // Collect and write storage history with proper shard management
        if ctx.storage_settings.storages_history_in_rocksdb {
            let mut storage_history: BTreeMap<(Address, B256), Vec<u64>> = BTreeMap::new();
            for (block_idx, block) in blocks.iter().enumerate() {
                let block_number = ctx.first_block_number + block_idx as u64;
                let bundle = &block.execution_outcome().bundle;
                for (&address, account) in bundle.state() {
                    for (&slot, _) in &account.storage {
                        let key = B256::new(slot.to_be_bytes());
                        storage_history.entry((address, key)).or_default().push(block_number);
                    }
                }
            }

            // Write storage history using proper shard append logic
            for ((address, slot), indices) in storage_history {
                batch.append_storage_history_shard(address, slot, indices)?;
            }
        }

        Ok(Some(batch.into_inner()))
    }

    /// Creates a read error from a `RocksDB` error.
    fn read_error(&self, e: rocksdb::Error) -> ProviderError {
        ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
            message: e.to_string().into(),
            code: -1,
        }))
    }

    /// Creates a delete error from a `RocksDB` error.
    fn delete_error(&self, e: rocksdb::Error) -> ProviderError {
        ProviderError::Database(DatabaseError::Delete(DatabaseErrorInfo {
            message: e.to_string().into(),
            code: -1,
        }))
    }

    /// Decodes a key-value pair from raw bytes.
    fn decode_kv<T: Table>(
        &self,
        key_bytes: &[u8],
        value_bytes: &[u8],
    ) -> ProviderResult<Option<(T::Key, T::Value)>> {
        let key = <T::Key as reth_db_api::table::Decode>::decode(key_bytes)
            .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
        let value = T::Value::decompress(value_bytes)
            .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
        Ok(Some((key, value)))
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
        let value_bytes = compress_to_buf_or_ref!(self.buf, value);
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

    /// Generic helper for appending indices to a history shard table.
    ///
    /// Loads the existing shard (if any), appends new indices, and rechunks into
    /// multiple shards if needed (respecting `NUM_OF_INDICES_IN_SHARD` limit).
    fn append_history_shard<T, F>(
        &mut self,
        last_key: T::Key,
        indices: impl IntoIterator<Item = u64>,
        make_key: F,
    ) -> ProviderResult<()>
    where
        T: Table<Value = BlockNumberList>,
        T::Key: Clone,
        F: Fn(u64) -> T::Key,
    {
        let last_shard_opt = self.provider.get::<T>(last_key.clone())?;
        let mut last_shard = last_shard_opt.unwrap_or_else(BlockNumberList::empty);

        last_shard.append(indices).map_err(ProviderError::other)?;

        // Fast path: all indices fit in one shard
        if last_shard.len() <= NUM_OF_INDICES_IN_SHARD as u64 {
            self.put::<T>(last_key, &last_shard)?;
            return Ok(());
        }

        // Slow path: rechunk into multiple shards
        let chunks = last_shard.iter().chunks(NUM_OF_INDICES_IN_SHARD);
        let mut chunks_peekable = chunks.into_iter().peekable();

        while let Some(chunk) = chunks_peekable.next() {
            let shard = BlockNumberList::new_pre_sorted(chunk);
            let highest = if chunks_peekable.peek().is_some() {
                shard.iter().next_back().expect("`chunks` does not return empty list")
            } else {
                u64::MAX
            };
            self.put::<T>(make_key(highest), &shard)?;
        }

        Ok(())
    }

    /// Appends indices to an account history shard with proper shard management.
    pub fn append_account_history_shard(
        &mut self,
        address: Address,
        indices: impl IntoIterator<Item = u64>,
    ) -> ProviderResult<()> {
        self.append_history_shard::<tables::AccountsHistory, _>(
            ShardedKey::new(address, u64::MAX),
            indices,
            |highest| ShardedKey::new(address, highest),
        )
    }

    /// Appends indices to a storage history shard with proper shard management.
    pub fn append_storage_history_shard(
        &mut self,
        address: Address,
        storage_key: B256,
        indices: impl IntoIterator<Item = u64>,
    ) -> ProviderResult<()> {
        self.append_history_shard::<tables::StoragesHistory, _>(
            StorageShardedKey::last(address, storage_key),
            indices,
            |highest| StorageShardedKey::new(address, storage_key, highest),
        )
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
/// `DbTx`/`DbTxMut` traits directly; use `RocksDB`-specific methods instead.
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
        let value_bytes = compress_to_buf_or_ref!(buf, value);

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
        let cf = self.provider.0.db.cf_handle(T::NAME).ok_or_else(|| {
            ProviderError::Database(DatabaseError::Other(format!(
                "column family not found: {}",
                T::NAME
            )))
        })?;

        // Create a raw iterator to access key bytes directly.
        let mut iter: DBRawIteratorWithThreadMode<'_, Transaction<'_, OptimisticTransactionDB>> =
            self.inner.raw_iterator_cf(&cf);

        // Seek to the smallest key >= encoded_key.
        iter.seek(encoded_key);
        Self::raw_iter_status_ok(&iter)?;

        if !iter.valid() {
            return Ok(history_not_found(lowest_available_block_number));
        }

        // Check if the found key matches our target entity.
        let Some(key_bytes) = iter.key() else {
            return Ok(history_not_found(lowest_available_block_number));
        };
        if !key_matches(key_bytes)? {
            return Ok(history_not_found(lowest_available_block_number));
        }

        // Decompress the block list for this shard.
        let Some(value_bytes) = iter.value() else {
            return Ok(history_not_found(lowest_available_block_number));
        };
        let chunk = BlockNumberList::decompress(value_bytes)?;

        // Get the rank of the first entry before or equal to our block.
        let mut rank = chunk.rank(block_number);

        // Adjust the rank, so that we have the rank of the first entry strictly before our
        // block (not equal to it).
        if rank.checked_sub(1).and_then(|r| chunk.select(r)) == Some(block_number) {
            rank -= 1;
        }

        let found_block = chunk.select(rank);

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

/// Iterator over a `RocksDB` table (non-transactional).
///
/// Yields decoded `(Key, Value)` pairs in key order.
pub struct RocksDBIter<'db, T: Table> {
    inner: rocksdb::DBIteratorWithThreadMode<'db, OptimisticTransactionDB>,
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
        decode_iter_result::<T>(self.inner.next()?)
    }
}

/// Reverse iterator over a `RocksDB` table (non-transactional).
///
/// Yields decoded `(Key, Value)` pairs in reverse key order.
pub struct RocksDBIterReverse<'db, T: Table> {
    inner: rocksdb::DBIteratorWithThreadMode<'db, OptimisticTransactionDB>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Table> fmt::Debug for RocksDBIterReverse<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDBIterReverse").field("table", &T::NAME).finish_non_exhaustive()
    }
}

impl<T: Table> Iterator for RocksDBIterReverse<'_, T> {
    type Item = ProviderResult<(T::Key, T::Value)>;

    fn next(&mut self) -> Option<Self::Item> {
        decode_iter_result::<T>(self.inner.next()?)
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
        decode_iter_result::<T>(self.inner.next()?)
    }
}

/// Helper function to decode iterator results.
///
/// Shared logic for all iterator implementations to avoid code duplication.
#[allow(clippy::type_complexity)]
fn decode_iter_result<T: Table>(
    result: Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>,
) -> Option<ProviderResult<(T::Key, T::Value)>> {
    match result {
        Ok((key_bytes, value_bytes)) => {
            let key = match <T::Key as reth_db_api::table::Decode>::decode(&key_bytes) {
                Ok(k) => k,
                Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
            };

            let value = match T::Value::decompress(&value_bytes) {
                Ok(v) => v,
                Err(_) => return Some(Err(ProviderError::Database(DatabaseError::Decode))),
            };

            Some(Ok((key, value)))
        }
        Err(e) => Some(Err(ProviderError::Database(DatabaseError::Read(DatabaseErrorInfo {
            message: e.to_string().into(),
            code: -1,
        })))),
    }
}

/// Returns the appropriate `HistoryInfo` when no matching history shard is found.
///
/// If `lowest_available_block_number` is set (pruning is active), returns `MaybeInPlainState`
/// since data may have been pruned. Otherwise, returns `NotYetWritten`.
const fn history_not_found(lowest_available_block_number: Option<BlockNumber>) -> HistoryInfo {
    if lowest_available_block_number.is_some() {
        HistoryInfo::MaybeInPlainState
    } else {
        HistoryInfo::NotYetWritten
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
    use crate::providers::HistoryInfo;
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

    #[derive(Debug)]
    struct TestTable2;

    impl Table for TestTable2 {
        const NAME: &'static str = "TestTable2";
        const DUPSORT: bool = false;
        type Key = u64;
        type Value = u64;
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

    #[test]
    fn test_account_history_info_single_shard() {
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
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address1 = Address::from([0x42; 20]);
        let address2 = Address::from([0x43; 20]);

        // Only add history for address1
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = ShardedKey::new(address1, u64::MAX);
        provider.put::<tables::AccountsHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query for address2 (no history exists): should return NotYetWritten
        let result = tx.account_history_info(address2, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        tx.rollback().unwrap();
    }

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
    fn test_storage_history_info() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path()).with_default_tables().build().unwrap();

        let address = Address::from([0x42; 20]);
        let storage_key = B256::from([0x01; 32]);

        // Create a single shard for this storage slot
        let chunk = IntegerList::new([100, 200, 300]).unwrap();
        let shard_key = StorageShardedKey::new(address, storage_key, u64::MAX);
        provider.put::<tables::StoragesHistory>(shard_key, &chunk).unwrap();

        let tx = provider.tx();

        // Query for block 150: should find block 200 in changeset
        let result = tx.storage_history_info(address, storage_key, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::InChangeset(200));

        // Query for block 50: should return NotYetWritten
        let result = tx.storage_history_info(address, storage_key, 50, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        // Query for block 500: should return InPlainState
        let result = tx.storage_history_info(address, storage_key, 500, None).unwrap();
        assert_eq!(result, HistoryInfo::InPlainState);

        // Query for different storage key (no history): should return NotYetWritten
        let other_key = B256::from([0x02; 32]);
        let result = tx.storage_history_info(address, other_key, 150, None).unwrap();
        assert_eq!(result, HistoryInfo::NotYetWritten);

        tx.rollback().unwrap();
    }

    #[test]
    fn test_clear_empty_table() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Clear an empty table should succeed without error
        provider.clear::<TestTable>().unwrap();

        // Verify table is still usable after clear
        provider.put::<TestTable>(1, &b"value".to_vec()).unwrap();
        assert_eq!(provider.get::<TestTable>(1).unwrap(), Some(b"value".to_vec()));
    }

    #[test]
    fn test_clear_single_entry() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert a single entry
        provider.put::<TestTable>(42, &b"test_value".to_vec()).unwrap();
        assert_eq!(provider.get::<TestTable>(42).unwrap(), Some(b"test_value".to_vec()));

        // Clear the table
        provider.clear::<TestTable>().unwrap();

        // Verify entry is gone
        assert_eq!(provider.get::<TestTable>(42).unwrap(), None);
    }

    #[test]
    fn test_clear_multiple_entries() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert multiple entries
        for i in 0..100u64 {
            provider.put::<TestTable>(i, &format!("value_{i}").into_bytes()).unwrap();
        }

        // Verify entries exist
        assert_eq!(provider.get::<TestTable>(50).unwrap(), Some(b"value_50".to_vec()));

        // Clear the table
        provider.clear::<TestTable>().unwrap();

        // Verify all entries are gone
        for i in 0..100u64 {
            assert_eq!(provider.get::<TestTable>(i).unwrap(), None);
        }

        // Verify table is still usable
        provider.put::<TestTable>(999, &b"new_value".to_vec()).unwrap();
        assert_eq!(provider.get::<TestTable>(999).unwrap(), Some(b"new_value".to_vec()));
    }

    #[test]
    fn test_iter_from_reverse_empty_table() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Reverse iterator on empty table should return no elements
        let iter = provider.iter_from_reverse::<TestTable>(100).unwrap();
        let items: Vec<_> = iter.collect();
        assert!(items.is_empty());
    }

    #[test]
    fn test_iter_from_reverse_single_entry() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert a single entry
        provider.put::<TestTable>(50, &b"value_50".to_vec()).unwrap();

        // Reverse iterator starting at/above the key should find it
        let iter = provider.iter_from_reverse::<TestTable>(100).unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], (50, b"value_50".to_vec()));

        // Reverse iterator starting below the key should not find it
        let iter = provider.iter_from_reverse::<TestTable>(40).unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_iter_from_reverse_multiple_entries() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert entries at keys 10, 20, 30, 40, 50
        for i in [10u64, 20, 30, 40, 50] {
            provider.put::<TestTable>(i, &format!("value_{i}").into_bytes()).unwrap();
        }

        // Reverse iterator starting at 35 should return 30, 20, 10 in that order
        let iter = provider.iter_from_reverse::<TestTable>(35).unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].0, 30);
        assert_eq!(items[1].0, 20);
        assert_eq!(items[2].0, 10);

        // Reverse iterator starting at 50 should return 50, 40, 30, 20, 10
        let iter = provider.iter_from_reverse::<TestTable>(50).unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].0, 50);
        assert_eq!(items[4].0, 10);
    }

    #[test]
    fn test_iter_from_reverse_key_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // Insert entries at keys 10, 30, 50
        provider.put::<TestTable>(10, &b"value_10".to_vec()).unwrap();
        provider.put::<TestTable>(30, &b"value_30".to_vec()).unwrap();
        provider.put::<TestTable>(50, &b"value_50".to_vec()).unwrap();

        // Starting at key 40 (doesn't exist) should start from 30
        let iter = provider.iter_from_reverse::<TestTable>(40).unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, 30);
        assert_eq!(items[1].0, 10);
    }

    // ========================
    // Migration Path Tests
    // ========================

    /// Test that creating a new `RocksDB` in an existing directory works.
    /// This simulates the migration path where MDBX-only data exists but
    /// `RocksDB` is being enabled for the first time.
    #[test]
    fn test_create_rocksdb_in_existing_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create some files to simulate existing data directory
        std::fs::write(temp_dir.path().join("dummy_file.txt"), b"existing data").unwrap();

        // Creating RocksDB should succeed even with existing directory contents
        let provider =
            RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();

        // RocksDB should be empty (no migration of data)
        let iter = provider.iter::<TestTable>().unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(items.is_empty());

        // Should be fully functional
        provider.put::<TestTable>(1, &b"new_data".to_vec()).unwrap();
        assert_eq!(provider.get::<TestTable>(1).unwrap(), Some(b"new_data".to_vec()));
    }

    /// Test that reopening `RocksDB` with additional column families works.
    /// This simulates adding support for new tables in a future version.
    #[test]
    fn test_open_rocksdb_add_new_column_family() {
        let temp_dir = TempDir::new().unwrap();

        // Create RocksDB with only TestTable
        {
            let provider =
                RocksDBBuilder::new(temp_dir.path()).with_table::<TestTable>().build().unwrap();
            provider.put::<TestTable>(42, &b"test_value".to_vec()).unwrap();
        }

        // Reopen with both TestTable and TestTable2
        // The new column family should be created automatically
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<TestTable>()
            .with_table::<TestTable2>()
            .build()
            .unwrap();

        // Original data should still be present
        assert_eq!(provider.get::<TestTable>(42).unwrap(), Some(b"test_value".to_vec()));

        // New table should be usable
        provider.put::<TestTable2>(100, &200u64).unwrap();
        assert_eq!(provider.get::<TestTable2>(100).unwrap(), Some(200u64));
    }

    // ========================
    // Large Data Volume Tests
    // ========================

    /// Test handling of large history shards with many block numbers.
    #[test]
    fn test_large_history_shard_performance() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Create a large shard with 10k block numbers
        let address = Address::from([0x42; 20]);
        let block_numbers: Vec<u64> = (0..10_000).collect();
        let shard = BlockNumberList::new_pre_sorted(block_numbers);
        let key = ShardedKey::new(address, u64::MAX);

        // Write the large shard
        let start = std::time::Instant::now();
        provider.put::<tables::AccountsHistory>(key.clone(), &shard).unwrap();
        let write_duration = start.elapsed();

        // Read the large shard
        let start = std::time::Instant::now();
        let result = provider.get::<tables::AccountsHistory>(key).unwrap();
        let read_duration = start.elapsed();

        assert!(result.is_some());
        let retrieved_shard = result.unwrap();
        assert_eq!(retrieved_shard.len(), 10_000);

        // Verify performance is reasonable (under 1 second each)
        assert!(write_duration.as_secs() < 1, "Write took too long: {:?}", write_duration);
        assert!(read_duration.as_secs() < 1, "Read took too long: {:?}", read_duration);
    }

    /// Test iterator performance with many entries across multiple addresses.
    #[test]
    fn test_iterator_large_dataset() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert 1000 entries across different addresses
        let entry_count = 1000u64;
        for i in 0..entry_count {
            let address = Address::from_slice(&[
                (i >> 8) as u8,
                (i & 0xFF) as u8,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ]);
            let shard = BlockNumberList::new_pre_sorted(vec![i, i + 1000, i + 2000]);
            let key = ShardedKey::new(address, u64::MAX);
            provider.put::<tables::AccountsHistory>(key, &shard).unwrap();
        }

        // Iterate all entries and count
        let start = std::time::Instant::now();
        let iter = provider.iter::<tables::AccountsHistory>().unwrap();
        let items: Vec<_> = iter.collect::<Result<Vec<_>, _>>().unwrap();
        let iter_duration = start.elapsed();

        assert_eq!(items.len(), entry_count as usize);
        assert!(iter_duration.as_secs() < 5, "Iterator took too long: {:?}", iter_duration);
    }
}
