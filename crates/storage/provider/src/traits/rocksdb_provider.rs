use crate::{
    either_writer::{RawRocksDBBatch, RocksBatchArg, RocksTxRefArg},
    providers::RocksDBProvider,
};
#[cfg(not(all(unix, feature = "rocksdb")))]
use reth_errors::ProviderError;
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

    /// Registers a raw `RocksDB` batch for deferred commit.
    ///
    /// This is a cfg-free wrapper that handles the batch registration internally.
    /// When rocksdb feature is disabled, this is a no-op.
    fn register_raw_rocksdb_batch(&self, batch: Option<RawRocksDBBatch>) {
        #[cfg(all(unix, feature = "rocksdb"))]
        if let Some(b) = batch {
            self.set_pending_rocksdb_batch(b);
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let _ = batch;
    }

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

    /// Executes a closure with a `RocksDB` batch if the feature is enabled.
    ///
    /// This helper validates that when `enabled` is true, the rocksdb feature is compiled.
    /// Returns an error if rocksdb is requested but the feature is missing.
    ///
    /// The closure receives a `RocksBatchArg` which is either a real batch or `()`.
    /// After the closure runs, the returned `RawRocksDBBatch` (if any) is automatically
    /// registered for deferred commit.
    fn with_rocksdb_batch_if<F, R>(
        &self,
        _enabled: bool,
        _feature_name: &'static str,
        f: F,
    ) -> ProviderResult<R>
    where
        F: FnOnce(RocksBatchArg<'_>) -> ProviderResult<(R, Option<RawRocksDBBatch>)>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            let rocksdb = self.rocksdb_provider();
            let batch = rocksdb.batch();
            let (result, raw_batch) = f(batch)?;
            self.register_raw_rocksdb_batch(raw_batch);
            Ok(result)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        {
            if _enabled {
                return Err(ProviderError::other(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "{_feature_name} is enabled but this binary was built without the 'rocksdb' feature"
                    ),
                )));
            }
            let (result, _) = f(())?;
            Ok(result)
        }
    }

    /// Executes a closure with a `RocksDB` transaction if the feature is enabled.
    ///
    /// This helper validates that when `enabled` is true, the rocksdb feature is compiled.
    /// Returns an error if rocksdb is requested but the feature is missing.
    fn with_rocksdb_tx_if<F, R>(
        &self,
        _enabled: bool,
        _feature_name: &'static str,
        f: F,
    ) -> ProviderResult<R>
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
        {
            if _enabled {
                return Err(ProviderError::other(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "{_feature_name} is enabled but this binary was built without the 'rocksdb' feature"
                    ),
                )));
            }
            f(())
        }
    }
}
