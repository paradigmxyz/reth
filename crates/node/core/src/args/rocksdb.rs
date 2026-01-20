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
    #[arg(long = "rocksdb.tx-hash", action = ArgAction::Set)]
    pub tx_hash: Option<bool>,

    /// Route storages history tables to `RocksDB` instead of MDBX.
    #[arg(long = "rocksdb.storages-history", action = ArgAction::Set)]
    pub storages_history: Option<bool>,

    /// Route account history tables to `RocksDB` instead of MDBX.
    #[arg(long = "rocksdb.account-history", action = ArgAction::Set)]
    pub account_history: Option<bool>,
}

impl RocksDbArgs {
    /// Applies CLI overrides to the given defaults, returning the resulting settings.
    ///
    /// If any flag is set to `true`, all `RocksDB` tables are enabled by default, then explicit
    /// `false` overrides are applied. This grouped behavior ensures users get the full `RocksDB`
    /// benefit without needing to specify all flags.
    ///
    /// Returns `(settings, grouped_enabled)` where `grouped_enabled` is true if the grouped
    /// behavior was triggered (any flag was true, enabling all tables).
    pub fn apply_to_settings(&self, mut settings: StorageSettings) -> (StorageSettings, bool) {
        let any_true = self.tx_hash == Some(true) ||
            self.storages_history == Some(true) ||
            self.account_history == Some(true);

        if any_true {
            settings.transaction_hash_numbers_in_rocksdb = true;
            settings.storages_history_in_rocksdb = true;
            settings.account_history_in_rocksdb = true;
        }

        if let Some(value) = self.tx_hash {
            settings.transaction_hash_numbers_in_rocksdb = value;
        }
        if let Some(value) = self.storages_history {
            settings.storages_history_in_rocksdb = value;
        }
        if let Some(value) = self.account_history {
            settings.account_history_in_rocksdb = value;
        }

        (settings, any_true)
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
    fn test_apply_to_settings_grouped_enable() {
        let args =
            RocksDbArgs { tx_hash: Some(true), storages_history: None, account_history: None };
        let (result, grouped) = args.apply_to_settings(StorageSettings::legacy());

        assert!(grouped, "should trigger grouped behavior when any flag is true");
        assert!(result.transaction_hash_numbers_in_rocksdb);
        assert!(result.storages_history_in_rocksdb, "grouped: all tables enabled");
        assert!(result.account_history_in_rocksdb, "grouped: all tables enabled");
    }

    #[test]
    fn test_apply_to_settings_explicit_disable() {
        let args = RocksDbArgs {
            tx_hash: Some(true),
            storages_history: Some(false),
            account_history: None,
        };
        let (result, grouped) = args.apply_to_settings(StorageSettings::legacy());

        assert!(grouped);
        assert!(result.transaction_hash_numbers_in_rocksdb);
        assert!(!result.storages_history_in_rocksdb, "explicit false overrides grouped enable");
        assert!(result.account_history_in_rocksdb);
    }

    #[test]
    fn test_apply_to_settings_no_grouped_when_all_none() {
        let args = RocksDbArgs::default();
        let (result, grouped) = args.apply_to_settings(StorageSettings::legacy());

        assert!(!grouped, "no grouped behavior when no flags set");
        assert!(!result.transaction_hash_numbers_in_rocksdb);
        assert!(!result.storages_history_in_rocksdb);
        assert!(!result.account_history_in_rocksdb);
    }

    #[test]
    fn test_apply_to_settings_all_explicit_false() {
        let args = RocksDbArgs {
            tx_hash: Some(false),
            storages_history: Some(false),
            account_history: Some(false),
        };
        let (result, grouped) = args.apply_to_settings(StorageSettings::legacy());

        assert!(!grouped, "no grouped when all explicit false");
        assert!(!result.transaction_hash_numbers_in_rocksdb);
        assert!(!result.storages_history_in_rocksdb);
        assert!(!result.account_history_in_rocksdb);
    }

    #[test]
    fn test_apply_to_settings_preserves_non_rocksdb_fields() {
        let base = StorageSettings::legacy().with_receipts_in_static_files(true);
        let args =
            RocksDbArgs { tx_hash: Some(true), storages_history: None, account_history: None };
        let (result, _) = args.apply_to_settings(base);

        assert!(result.receipts_in_static_files, "non-rocksdb settings preserved");
    }
}
