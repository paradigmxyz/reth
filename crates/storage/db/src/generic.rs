//! Generic database environment that supports multiple backends.
// Re-export the unified database functions and types
pub use crate::database::{create_db, init_db, open_db, open_db_read_only};

// Re-export backend-specific types based on enabled features
// MDBX and RocksDB are mutually exclusive features
#[cfg(all(feature = "mdbx", not(feature = "rocksdb")))]
pub use crate::implementation::mdbx::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};

#[cfg(all(feature = "rocksdb", not(feature = "mdbx")))]
pub use crate::implementation::rocksdb::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};

// When both features are enabled, prefer rocksdb
#[cfg(all(feature = "mdbx", feature = "rocksdb"))]
pub use crate::implementation::rocksdb::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};
