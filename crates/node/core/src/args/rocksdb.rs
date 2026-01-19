//! clap [Args](clap::Args) for RocksDB table routing configuration

use clap::{ArgAction, Args};
use reth_storage_api::StorageSettings;

/// Parameters for RocksDB table routing configuration.
///
/// These flags control which database tables are stored in RocksDB instead of MDBX.
/// All flags are genesis-initialization-only: changing them after genesis requires a re-sync.
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "RocksDB")]
pub struct RocksDbArgs {
    /// Route tx hash -> number table to RocksDB instead of MDBX.
    ///
    /// Note: genesis-initialization-only, changing later requires re-sync.
    #[arg(long = "rocksdb.tx-hash", action = ArgAction::Set)]
    pub tx_hash: Option<bool>,

    /// Route storages history tables to RocksDB instead of MDBX.
    ///
    /// Note: genesis-initialization-only, changing later requires re-sync.
    #[arg(long = "rocksdb.storages-history", action = ArgAction::Set)]
    pub storages_history: Option<bool>,

    /// Route account history tables to RocksDB instead of MDBX.
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
    pub fn apply_to_settings(&self, mut defaults: StorageSettings) -> StorageSettings {
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

    /// Returns true if any RocksDB table routing flag was explicitly set.
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
        let default_args = RocksDbArgs::default();
        let args = CommandParser::<RocksDbArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
        assert_eq!(args.tx_hash, None);
        assert_eq!(args.storages_history, None);
        assert_eq!(args.account_history, None);
    }

    #[test]
    fn test_rocksdb_tx_hash_true() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.tx-hash=true"]).args;
        assert_eq!(args.tx_hash, Some(true));
    }

    #[test]
    fn test_rocksdb_tx_hash_false() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.tx-hash=false"]).args;
        assert_eq!(args.tx_hash, Some(false));
    }

    #[test]
    fn test_rocksdb_storages_history_true() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.storages-history=true"])
                .args;
        assert_eq!(args.storages_history, Some(true));
    }

    #[test]
    fn test_rocksdb_storages_history_false() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.storages-history=false"])
                .args;
        assert_eq!(args.storages_history, Some(false));
    }

    #[test]
    fn test_rocksdb_account_history_true() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.account-history=true"])
                .args;
        assert_eq!(args.account_history, Some(true));
    }

    #[test]
    fn test_rocksdb_account_history_false() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.account-history=false"])
                .args;
        assert_eq!(args.account_history, Some(false));
    }

    #[test]
    fn test_all_flags_together() {
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
        let args = RocksDbArgs {
            tx_hash: Some(true),
            storages_history: None,
            account_history: Some(false),
        };

        let defaults = StorageSettings::legacy();
        let result = args.apply_to_settings(defaults);

        assert!(result.transaction_hash_numbers_in_rocksdb);
        assert!(!result.storages_history_in_rocksdb);
        assert!(!result.account_history_in_rocksdb);
    }

    #[test]
    fn test_has_overrides() {
        let empty = RocksDbArgs::default();
        assert!(!empty.has_overrides());

        let with_tx_hash = RocksDbArgs { tx_hash: Some(true), ..Default::default() };
        assert!(with_tx_hash.has_overrides());

        let with_all = RocksDbArgs {
            tx_hash: Some(true),
            storages_history: Some(false),
            account_history: Some(true),
        };
        assert!(with_all.has_overrides());
    }

    #[test]
    fn test_bare_flag_requires_value() {
        let result = CommandParser::<RocksDbArgs>::try_parse_from(["reth", "--rocksdb.tx-hash"]);
        assert!(result.is_err(), "bare flag without value should require a value");
    }

    #[test]
    fn test_space_separated_value() {
        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.tx-hash", "true"]).args;
        assert_eq!(args.tx_hash, Some(true));

        let args =
            CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.tx-hash", "false"]).args;
        assert_eq!(args.tx_hash, Some(false));
    }
}
