use crate::providers::RocksDBProvider;

/// `RocksDB` provider factory.
///
/// This trait provides access to the `RocksDB` provider
pub trait RocksDBProviderFactory {
    /// Returns the `RocksDB` provider.
    fn rocksdb_provider(&self) -> RocksDBProvider;

    /// Adds a pending `RocksDB` batch to be committed when this provider is committed.
    ///
    /// This allows deferring `RocksDB` commits to happen at the same time as MDBX and static file
    /// commits, ensuring atomicity across all storage backends.
    #[cfg(all(unix, feature = "rocksdb"))]
    fn set_pending_rocksdb_batch(&self, batch: rocksdb::WriteBatchWithTransaction<true>);
}
