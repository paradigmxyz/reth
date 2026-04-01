use crate::{
    either_writer::{RawRocksDBBatch, RocksBatchArg, RocksDBRefArg},
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
    fn set_pending_rocksdb_batch(&self, batch: rocksdb::WriteBatchWithTransaction<true>);

    /// Takes all pending `RocksDB` batches and commits them.
    ///
    /// This drains the pending batches from the lock and commits each one using the `RocksDB`
    /// provider. Can be called before flush to persist `RocksDB` writes independently of the
    /// full commit path.
    fn commit_pending_rocksdb_batches(&self) -> ProviderResult<()>;

    /// Executes a closure with a `RocksDB` point-in-time snapshot for consistent reads.
    ///
    /// This helper encapsulates `RocksDB` access for read operations.
    /// On legacy MDBX-only nodes (where `storage_v2` is false), this skips creating
    /// the `RocksDB` snapshot entirely, avoiding unnecessary overhead.
    ///
    /// Unlike a transaction-based approach, this works in both read-only and read-write
    /// modes since the snapshot provides a consistent view of the data at the time it
    /// was created.
    fn with_rocksdb_snapshot<F, R>(&self, f: F) -> ProviderResult<R>
    where
        Self: StorageSettingsCache,
        F: FnOnce(RocksDBRefArg<'_>) -> ProviderResult<R>,
    {
        if self.cached_storage_settings().storage_v2 {
            let rocksdb = self.rocksdb_provider();
            let snapshot = rocksdb.snapshot();
            return f(Some(snapshot));
        }
        f(None)
    }

    /// Executes a closure with a `RocksDB` batch, automatically registering it for commit.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` batch handling.
    fn with_rocksdb_batch<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksBatchArg<'_>) -> ProviderResult<(R, Option<RawRocksDBBatch>)>,
    {
        let rocksdb = self.rocksdb_provider();
        let batch = rocksdb.batch();
        let (result, raw_batch) = f(batch)?;
        if let Some(b) = raw_batch {
            self.set_pending_rocksdb_batch(b);
        }
        Ok(result)
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
        let rocksdb = self.rocksdb_provider();
        let batch = rocksdb.batch_with_auto_commit();
        let (result, raw_batch) = f(batch)?;
        if let Some(b) = raw_batch {
            self.set_pending_rocksdb_batch(b);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::models::StorageSettings;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock `RocksDB` provider that tracks snapshot creation calls.
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
    fn test_legacy_settings_skip_rocksdb_snapshot() {
        let provider = TestProvider::new(StorageSettings::v1());

        let result = provider.with_rocksdb_snapshot(|rocksdb| {
            assert!(rocksdb.is_none(), "legacy settings should pass None");
            Ok(42)
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            provider.tx_call_count(),
            0,
            "should not create RocksDB provider for legacy settings"
        );
    }

    #[test]
    fn test_rocksdb_settings_create_snapshot() {
        let settings = StorageSettings::v2();
        let provider = TestProvider::new(settings);

        let result = provider.with_rocksdb_snapshot(|rocksdb| {
            assert!(rocksdb.is_some(), "rocksdb settings should pass Some snapshot");
            Ok(42)
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            provider.tx_call_count(),
            1,
            "should create RocksDB provider when storage_v2 is true"
        );
    }
}
