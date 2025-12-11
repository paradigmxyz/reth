//! `RocksDB` provider factory trait.

#[cfg(all(unix, feature = "rocksdb"))]
use crate::providers::RocksDBProvider;

/// `RocksDB` provider factory.
///
/// This trait provides access to a [`RocksDBProvider`] for reading and writing
/// to `RocksDB`-backed tables (e.g., `AccountsHistory`, `StoragesHistory`,
/// `TransactionHashNumbers`).
#[cfg(all(unix, feature = "rocksdb"))]
pub trait RocksDBProviderFactory {
    /// Returns a clone of the `RocksDB` provider.
    fn rocksdb_provider(&self) -> RocksDBProvider;
}
