//! clap [Args](clap::Args) for storage configuration (static files + RocksDB tables)

use clap::Args;
use reth_provider::StorageSettings;

use super::StaticFilesArgs;

/// Parameters for storage configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Static files related flags.
    #[command(flatten)]
    pub static_files: StaticFilesArgs,

    /// Store transaction hash -> number mapping in `RocksDB` instead of MDBX.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "storage.tx-hash-in-rocksdb")]
    pub tx_hash_in_rocksdb: bool,

    /// Store storages history in `RocksDB` instead of MDBX.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "storage.storages-history-in-rocksdb")]
    pub storages_history_in_rocksdb: bool,

    /// Store account history in `RocksDB` instead of MDBX.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "storage.account-history-in-rocksdb")]
    pub account_history_in_rocksdb: bool,
}

impl StorageArgs {
    /// Converts CLI storage arguments into [`StorageSettings`].
    pub const fn to_settings(&self) -> StorageSettings {
        self.static_files
            .to_settings()
            .with_transaction_hash_numbers_in_rocksdb(self.tx_hash_in_rocksdb)
            .with_storages_history_in_rocksdb(self.storages_history_in_rocksdb)
            .with_account_history_in_rocksdb(self.account_history_in_rocksdb)
    }
}
