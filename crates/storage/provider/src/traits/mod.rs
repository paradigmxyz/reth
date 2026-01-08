//! Collection of common provider traits.

// Re-export all the traits
pub use reth_storage_api::*;

pub use reth_chainspec::ChainSpecProvider;

mod static_file_provider;
pub use static_file_provider::StaticFileProviderFactory;

mod rocksdb_provider;
pub use rocksdb_provider::RocksDBProviderFactory;

mod triedb_provider;
pub use triedb_provider::TrieDBProviderFactory;

mod full;
pub use full::FullProvider;
