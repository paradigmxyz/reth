use crate::providers::RocksDBProvider;

/// `RocksDB` provider factory.
///
/// This trait provides access to the `RocksDB` provider
pub trait RocksDBProviderFactory {
    /// Returns the `RocksDB` provider.
    fn rocksdb_provider(&self) -> RocksDBProvider;
}
