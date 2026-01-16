use crate::{either_writer::RocksTxRefArg, providers::RocksDBProvider};
use reth_storage_errors::provider::ProviderResult;

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

    /// Executes a closure with a `RocksDB` transaction for reading.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` transaction handling for reads.
    fn with_rocksdb_tx<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksTxRefArg<'_>) -> ProviderResult<R>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            let rocksdb = self.rocksdb_provider();
            let tx = rocksdb.tx();
            f(&tx)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        f(())
    }
}
