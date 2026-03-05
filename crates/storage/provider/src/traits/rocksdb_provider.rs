use crate::{
    either_writer::{RawRocksDBBatch, RocksBatchArg, RocksTxRefArg},
    providers::RocksDBProvider,
};
use reth_storage_api::StorageSettingsCache;
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

    /// Takes all pending `RocksDB` batches and commits them.
    ///
    /// This drains the pending batches from the lock and commits each one using the `RocksDB`
    /// provider. Can be called before flush to persist `RocksDB` writes independently of the
    /// full commit path.
    #[cfg(all(unix, feature = "rocksdb"))]
    fn commit_pending_rocksdb_batches(&self) -> ProviderResult<()>;

    /// Executes a closure with a `RocksDB` transaction for reading.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` transaction handling for reads.
    /// On legacy MDBX-only nodes (where `any_in_rocksdb()` is false), this skips creating
    /// the `RocksDB` transaction entirely, avoiding unnecessary overhead.
    fn with_rocksdb_tx<F, R>(&self, f: F) -> ProviderResult<R>
    where
        Self: StorageSettingsCache,
        F: FnOnce(RocksTxRefArg<'_>) -> ProviderResult<R>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            if self.cached_storage_settings().storage_v2 {
                let rocksdb = self.rocksdb_provider();
                let tx = rocksdb.tx();
                return f(Some(&tx));
            }
            f(None)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        f(())
    }

    /// Executes a closure with a `RocksDB` batch, automatically registering it for commit.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` batch handling.
    fn with_rocksdb_batch<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksBatchArg<'_>) -> ProviderResult<(R, Option<RawRocksDBBatch>)>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            let rocksdb = self.rocksdb_provider();
            let batch = rocksdb.batch();
            let (result, raw_batch) = f(batch)?;
            if let Some(b) = raw_batch {
                self.set_pending_rocksdb_batch(b);
            }
            Ok(result)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        {
            let (result, _) = f(())?;
            Ok(result)
        }
    }

    /// Executes a closure with a `RocksDB` batch that auto-commits on threshold.
    ///
    /// Unlike [`Self::with_rocksdb_batch`], this uses a batch that automatically commits
    /// when it exceeds the size threshold, preventing OOM during large bulk writes.
    /// The consistency check on startup heals any crash between auto-commits.
    fn with_rocksdb_batch_auto_commit<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksBatchArg<'_>) -> ProviderResult<(R, Option<RawRocksDBBatch>)>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            let rocksdb = self.rocksdb_provider();
            let batch = rocksdb.batch_with_auto_commit();
            let (result, raw_batch) = f(batch)?;
            if let Some(b) = raw_batch {
                self.set_pending_rocksdb_batch(b);
            }
            Ok(result)
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        {
            let (result, _) = f(())?;
            Ok(result)
        }
    }
}

#[cfg(all(test, unix, feature = "rocksdb"))]
mod tests {
    use super::*;
    use reth_db_api::models::StorageSettings;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock `RocksDB` provider that tracks `tx()` calls.
    struct MockRocksDBProvider {
        tx_call_count: AtomicUsize,
    }

    impl MockRocksDBProvider {
        const fn new() -> Self {
            Self { tx_call_count: AtomicUsize::new(0) }
        }

        fn tx_call_count(&self) -> usize {
            self.tx_call_count.load(Ordering::SeqCst)
        }

        fn increment_tx_count(&self) {
            self.tx_call_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Test provider that implements [`RocksDBProviderFactory`] + [`StorageSettingsCache`].
    struct TestProvider {
        settings: StorageSettings,
        mock_rocksdb: MockRocksDBProvider,
        temp_dir: tempfile::TempDir,
    }

    impl TestProvider {
        fn new(settings: StorageSettings) -> Self {
            Self {
                settings,
                mock_rocksdb: MockRocksDBProvider::new(),
                temp_dir: tempfile::TempDir::new().unwrap(),
            }
        }

        fn tx_call_count(&self) -> usize {
            self.mock_rocksdb.tx_call_count()
        }
    }

    impl StorageSettingsCache for TestProvider {
        fn cached_storage_settings(&self) -> StorageSettings {
            self.settings
        }

        fn set_storage_settings_cache(&self, _settings: StorageSettings) {}
    }

    impl RocksDBProviderFactory for TestProvider {
        fn rocksdb_provider(&self) -> RocksDBProvider {
            self.mock_rocksdb.increment_tx_count();
            RocksDBProvider::new(self.temp_dir.path()).unwrap()
        }

        fn set_pending_rocksdb_batch(&self, _batch: rocksdb::WriteBatchWithTransaction<true>) {}

        fn commit_pending_rocksdb_batches(&self) -> ProviderResult<()> {
            Ok(())
        }
    }

    #[test]
    fn test_legacy_settings_skip_rocksdb_tx_creation() {
        let provider = TestProvider::new(StorageSettings::v1());

        let result = provider.with_rocksdb_tx(|tx| {
            assert!(tx.is_none(), "legacy settings should pass None tx");
            Ok(42)
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(provider.tx_call_count(), 0, "should not create RocksDB tx for legacy settings");
    }

    #[test]
    fn test_rocksdb_settings_create_tx() {
        let settings = StorageSettings::v2();
        let provider = TestProvider::new(settings);

        let result = provider.with_rocksdb_tx(|tx| {
            assert!(tx.is_some(), "rocksdb settings should pass Some tx");
            Ok(42)
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            provider.tx_call_count(),
            1,
            "should create RocksDB tx when any_in_rocksdb is true"
        );
    }
}
