use crate::providers::RocksDBProvider;

/// `RocksDB` provider factory.
///
/// This trait provides access to the `RocksDB` provider
pub trait RocksDBProviderFactory {
    /// Returns a reference to the `RocksDB` provider.
    fn rocksdb_provider(&self) -> &RocksDBProvider;
}
