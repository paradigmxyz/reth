//! clap [Args](clap::Args) for storage configuration (static files + RocksDB tables)

use clap::{ArgAction, Args};
use reth_provider::{StorageSettings, StorageSettingsOverrides};

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
    #[arg(long = "storage.tx-hash-in-rocksdb", action = ArgAction::Set)]
    pub tx_hash_in_rocksdb: Option<bool>,

    /// Store storages history in `RocksDB` instead of MDBX.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "storage.storages-history-in-rocksdb", action = ArgAction::Set)]
    pub storages_history_in_rocksdb: Option<bool>,

    /// Store account history in `RocksDB` instead of MDBX.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "storage.account-history-in-rocksdb", action = ArgAction::Set)]
    pub account_history_in_rocksdb: Option<bool>,
}

impl StorageArgs {
    /// Converts CLI storage arguments into [`StorageSettings`].
    ///
    /// Uses the provided default settings for any flags not explicitly set.
    pub const fn to_settings(&self, defaults: StorageSettings) -> StorageSettings {
        let base = self.static_files.to_settings();

        let tx_hash = match self.tx_hash_in_rocksdb {
            Some(v) => v,
            None => defaults.transaction_hash_numbers_in_rocksdb,
        };
        let storages_history = match self.storages_history_in_rocksdb {
            Some(v) => v,
            None => defaults.storages_history_in_rocksdb,
        };
        let account_history = match self.account_history_in_rocksdb {
            Some(v) => v,
            None => defaults.account_history_in_rocksdb,
        };

        base.with_transaction_hash_numbers_in_rocksdb(tx_hash)
            .with_storages_history_in_rocksdb(storages_history)
            .with_account_history_in_rocksdb(account_history)
    }

    /// Returns the overrides for RocksDB settings that were explicitly set via CLI.
    pub const fn to_overrides(&self) -> StorageSettingsOverrides {
        StorageSettingsOverrides {
            transaction_hash_numbers_in_rocksdb: self.tx_hash_in_rocksdb,
            storages_history_in_rocksdb: self.storages_history_in_rocksdb,
            account_history_in_rocksdb: self.account_history_in_rocksdb,
        }
    }
}
