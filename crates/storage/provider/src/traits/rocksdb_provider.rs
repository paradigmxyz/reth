use crate::{either_writer::RocksTxRefArg, providers::RocksDBProvider};
use reth_storage_api::StorageSettingsCache;
use reth_storage_errors::provider::ProviderResult;

/// `RocksDB` provider factory.
///
/// This trait provides access to the `RocksDB` provider
pub trait RocksDBProviderFactory: StorageSettingsCache {
    /// Returns the `RocksDB` provider.
    fn rocksdb_provider(&self) -> RocksDBProvider;

    /// Adds a pending `RocksDB` batch to be committed when this provider is committed.
    ///
    /// This allows deferring `RocksDB` commits to happen at the same time as MDBX and static file
    /// commits, ensuring atomicity across all storage backends.
    #[cfg(all(unix, feature = "rocksdb"))]
    fn set_pending_rocksdb_batch(&self, batch: rocksdb::WriteBatchWithTransaction<true>);

    /// Executes a closure with a `RocksDB` transaction for reading.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` transaction handling for reads.
    /// Only creates a `RocksDB` transaction when storage settings indicate `RocksDB` is needed
    /// for at least one table, avoiding overhead for legacy MDBX-only nodes.
    fn with_rocksdb_tx<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksTxRefArg<'_>) -> ProviderResult<R>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            if self.cached_storage_settings().any_in_rocksdb() {
                let rocksdb = self.rocksdb_provider();
                let tx = rocksdb.tx();
                return f(Some(&tx));
            }
            f(None)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        f(None)
    }
}
