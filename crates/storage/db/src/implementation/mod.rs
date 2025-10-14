// MDBX and RocksDB are mutually exclusive features
#[cfg(all(feature = "mdbx", not(feature = "rocksdb")))]
pub mod mdbx;

#[cfg(all(feature = "rocksdb", not(feature = "mdbx")))]
pub mod rocksdb;

// When both features are enabled, prefer rocksdb
#[cfg(all(feature = "mdbx", feature = "rocksdb"))]
pub mod rocksdb;
