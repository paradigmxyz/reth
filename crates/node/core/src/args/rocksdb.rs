//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};
use reth_storage_api::StorageSettings;

/// Parameters for `RocksDB` table routing configuration.
///
/// These flags control which database tables are stored in `RocksDB` instead of MDBX.
/// All flags are genesis-initialization-only: changing them after genesis requires a re-sync.
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "RocksDB")]
pub struct RocksDbArgs {
    /// Route tx hash -> number table to `RocksDB` instead of MDBX.
    ///
    /// Note: genesis-initialization-only, changing later requires re-sync.
    #[arg(long = "rocksdb.tx-hash", action = ArgAction::Set)]
    pub tx_hash: Option<bool>,

    /// Route storages history tables to `RocksDB` instead of MDBX.
    ///
    /// Note: genesis-initialization-only, changing later requires re-sync.
    #[arg(long = "rocksdb.storages-history", action = ArgAction::Set)]
    pub storages_history: Option<bool>,

    /// Route account history tables to `RocksDB` instead of MDBX.
    ///
    /// Note: genesis-initialization-only, changing later requires re-sync.
    #[arg(long = "rocksdb.account-history", action = ArgAction::Set)]
    pub account_history: Option<bool>,
}

impl RocksDbArgs {
    /// Applies CLI overrides to the given defaults, returning the resulting settings.
    ///
    /// For each flag that is `Some`, the corresponding field in the defaults is overwritten.
    /// Flags that are `None` preserve the default value.
    pub const fn apply_to_settings(&self, mut defaults: StorageSettings) -> StorageSettings {
        if let Some(value) = self.tx_hash {
            defaults.transaction_hash_numbers_in_rocksdb = value;
        }
        if let Some(value) = self.storages_history {
            defaults.storages_history_in_rocksdb = value;
        }
        if let Some(value) = self.account_history {
            defaults.account_history_in_rocksdb = value;
        }
        defaults
    }

    /// Returns true if any `RocksDB` table routing flag was explicitly set.
    pub const fn has_overrides(&self) -> bool {
        self.tx_hash.is_some() || self.storages_history.is_some() || self.account_history.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_default_rocksdb_args() {
        let args = CommandParser::<RocksDbArgs>::parse_from(["reth"]).args;
        assert_eq!(args, RocksDbArgs::default());
    }

    #[test]
    fn test_parse_all_flags() {
        let args = CommandParser::<RocksDbArgs>::parse_from([
            "reth",
            "--rocksdb.tx-hash=true",
            "--rocksdb.storages-history=false",
            "--rocksdb.account-history=true",
        ])
        .args;
        assert_eq!(args.tx_hash, Some(true));
        assert_eq!(args.storages_history, Some(false));
        assert_eq!(args.account_history, Some(true));
    }

    #[test]
    fn test_apply_to_settings() {
        let args =
            RocksDbArgs { tx_hash: Some(true), storages_history: None, account_history: None };
        let result = args.apply_to_settings(StorageSettings::legacy());
        assert!(result.transaction_hash_numbers_in_rocksdb);
        assert!(!result.storages_history_in_rocksdb);
    }
}
