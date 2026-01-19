//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};
use reth_storage_api::StorageSettings;
use std::fmt;

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

    /// Validates that CLI overrides match the persisted storage settings.
    ///
    /// This should be called at startup after loading the persisted settings but before
    /// the pipeline starts. If any CLI override differs from the persisted value,
    /// returns an error since these are genesis-initialization-only settings.
    ///
    /// Returns `Ok(())` if:
    /// - No CLI overrides are set (all `None`)
    /// - All CLI overrides match the persisted values
    ///
    /// Returns `Err` if any CLI override differs from the persisted value.
    pub const fn validate_against_persisted(
        &self,
        persisted: &StorageSettings,
    ) -> Result<(), RocksDbSettingsMismatchError> {
        if let Some(cli_value) = self.tx_hash &&
            cli_value != persisted.transaction_hash_numbers_in_rocksdb
        {
            return Err(RocksDbSettingsMismatchError {
                flag_name: "--rocksdb.tx-hash",
                expected: persisted.transaction_hash_numbers_in_rocksdb,
                got: cli_value,
            });
        }

        if let Some(cli_value) = self.storages_history &&
            cli_value != persisted.storages_history_in_rocksdb
        {
            return Err(RocksDbSettingsMismatchError {
                flag_name: "--rocksdb.storages-history",
                expected: persisted.storages_history_in_rocksdb,
                got: cli_value,
            });
        }

        if let Some(cli_value) = self.account_history &&
            cli_value != persisted.account_history_in_rocksdb
        {
            return Err(RocksDbSettingsMismatchError {
                flag_name: "--rocksdb.account-history",
                expected: persisted.account_history_in_rocksdb,
                got: cli_value,
            });
        }

        Ok(())
    }
}

/// Error returned when a CLI `RocksDB` flag differs from the persisted storage settings.
///
/// These settings are genesis-initialization-only and cannot be changed after the node
/// has been initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RocksDbSettingsMismatchError {
    /// The CLI flag name that mismatched.
    pub flag_name: &'static str,
    /// The expected value (from persisted settings).
    pub expected: bool,
    /// The value provided via CLI.
    pub got: bool,
}

impl fmt::Display for RocksDbSettingsMismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` differs from initialized layout (expected {}, got {}). \
             This setting is genesis-only; re-sync required (remove datadir or use a new datadir).",
            self.flag_name, self.expected, self.got
        )
    }
}

impl std::error::Error for RocksDbSettingsMismatchError {}

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

    #[test]
    fn test_validate_no_overrides_passes() {
        let args = RocksDbArgs::default();
        let persisted = StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true);
        assert!(args.validate_against_persisted(&persisted).is_ok());
    }

    #[test]
    fn test_validate_matching_overrides_passes() {
        let args = RocksDbArgs {
            tx_hash: Some(true),
            storages_history: Some(false),
            account_history: Some(false),
        };
        let persisted = StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true);
        assert!(args.validate_against_persisted(&persisted).is_ok());
    }

    #[test]
    fn test_validate_mismatched_tx_hash_fails() {
        let args = RocksDbArgs { tx_hash: Some(true), ..Default::default() };
        let persisted = StorageSettings::legacy();

        let err = args.validate_against_persisted(&persisted).unwrap_err();
        assert_eq!(err.flag_name, "--rocksdb.tx-hash");
        assert!(!err.expected);
        assert!(err.got);
        assert!(err.to_string().contains("--rocksdb.tx-hash"));
        assert!(err.to_string().contains("genesis-only"));
    }

    #[test]
    fn test_validate_mismatched_storages_history_fails() {
        let args = RocksDbArgs { storages_history: Some(false), ..Default::default() };
        let persisted = StorageSettings::legacy().with_storages_history_in_rocksdb(true);

        let err = args.validate_against_persisted(&persisted).unwrap_err();
        assert_eq!(err.flag_name, "--rocksdb.storages-history");
        assert!(err.expected);
        assert!(!err.got);
    }

    #[test]
    fn test_validate_mismatched_account_history_fails() {
        let args = RocksDbArgs { account_history: Some(true), ..Default::default() };
        let persisted = StorageSettings::legacy();

        let err = args.validate_against_persisted(&persisted).unwrap_err();
        assert_eq!(err.flag_name, "--rocksdb.account-history");
        assert!(!err.expected);
        assert!(err.got);
    }

    #[test]
    fn test_error_message_format() {
        let err = RocksDbSettingsMismatchError {
            flag_name: "--rocksdb.tx-hash",
            expected: false,
            got: true,
        };
        let msg = err.to_string();
        assert!(msg.contains("--rocksdb.tx-hash"));
        assert!(msg.contains("expected false"));
        assert!(msg.contains("got true"));
        assert!(msg.contains("genesis-only"));
        assert!(msg.contains("re-sync required"));
    }

    #[test]
    fn test_validate_matching_false_overrides_passes() {
        // Test that explicitly setting false matches persisted false
        let args = RocksDbArgs {
            tx_hash: Some(false),
            storages_history: Some(false),
            account_history: Some(false),
        };
        let persisted = StorageSettings::legacy(); // All rocksdb flags are false
        assert!(args.validate_against_persisted(&persisted).is_ok());
    }

    #[test]
    fn test_validate_multiple_mismatches_returns_first() {
        // When multiple fields mismatch, the first one (tx_hash) is reported
        let args = RocksDbArgs {
            tx_hash: Some(true),
            storages_history: Some(true),
            account_history: Some(true),
        };
        let persisted = StorageSettings::legacy(); // All false

        let err = args.validate_against_persisted(&persisted).unwrap_err();
        // Should report the first mismatch (tx_hash is checked first)
        assert_eq!(err.flag_name, "--rocksdb.tx-hash");
    }

    #[test]
    fn test_validate_partial_override_with_match() {
        // Only one CLI override set, and it matches
        let args = RocksDbArgs { storages_history: Some(true), ..Default::default() };
        let persisted = StorageSettings::legacy().with_storages_history_in_rocksdb(true);
        assert!(args.validate_against_persisted(&persisted).is_ok());
    }
}
