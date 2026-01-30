//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};
use reth_storage_api::StorageSettings;

/// Default value for `tx_hash` routing flag.
///
/// Derived from [`StorageSettings::base()`] to ensure CLI defaults match storage defaults.
const fn default_tx_hash_in_rocksdb() -> bool {
    StorageSettings::base().transaction_hash_numbers_in_rocksdb
}

/// Default value for `storages_history` routing flag.
///
/// Derived from [`StorageSettings::base()`] to ensure CLI defaults match storage defaults.
const fn default_storages_history_in_rocksdb() -> bool {
    StorageSettings::base().storages_history_in_rocksdb
}

/// Default value for `account_history` routing flag.
///
/// Derived from [`StorageSettings::base()`] to ensure CLI defaults match storage defaults.
const fn default_account_history_in_rocksdb() -> bool {
    StorageSettings::base().account_history_in_rocksdb
}

/// Default value for `account_changesets` routing flag.
///
/// Derived from [`StorageSettings::base()`] to ensure CLI defaults match storage defaults.
const fn default_account_changesets_in_rocksdb() -> bool {
    StorageSettings::base().account_changesets_in_rocksdb
}

/// Default value for `storage_changesets` routing flag.
///
/// Derived from [`StorageSettings::base()`] to ensure CLI defaults match storage defaults.
const fn default_storage_changesets_in_rocksdb() -> bool {
    StorageSettings::base().storage_changesets_in_rocksdb
}

/// Parameters for `RocksDB` table routing configuration.
///
/// These flags control which database tables are stored in `RocksDB` instead of MDBX.
/// All flags are genesis-initialization-only: changing them after genesis requires a re-sync.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy)]
#[command(next_help_heading = "RocksDB")]
pub struct RocksDbArgs {
    /// Route all supported tables to `RocksDB` instead of MDBX.
    ///
    /// This enables `RocksDB` for `tx-hash`, `storages-history`, and `account-history` tables.
    /// Cannot be combined with individual flags set to false.
    #[arg(long = "rocksdb.all", action = ArgAction::SetTrue)]
    pub all: bool,

    /// Route tx hash -> number table to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `true` when the `edge` feature is enabled, `false` otherwise.
    #[arg(long = "rocksdb.tx-hash", default_value_t = default_tx_hash_in_rocksdb(), action = ArgAction::Set)]
    pub tx_hash: bool,

    /// Route storages history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `false`.
    #[arg(long = "rocksdb.storages-history", default_value_t = default_storages_history_in_rocksdb(), action = ArgAction::Set)]
    pub storages_history: bool,

    /// Route account history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `false`.
    #[arg(long = "rocksdb.account-history", default_value_t = default_account_history_in_rocksdb(), action = ArgAction::Set)]
    pub account_history: bool,

    /// Route account changesets to `RocksDB` instead of MDBX or static files.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `true` when the `edge` feature is enabled, `false` otherwise.
    #[arg(long = "rocksdb.account-changesets", default_value_t = default_account_changesets_in_rocksdb(), action = ArgAction::Set)]
    pub account_changesets: bool,

    /// Route storage changesets to `RocksDB` instead of MDBX or static files.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `true` when the `edge` feature is enabled, `false` otherwise.
    #[arg(long = "rocksdb.storage-changesets", default_value_t = default_storage_changesets_in_rocksdb(), action = ArgAction::Set)]
    pub storage_changesets: bool,
}

impl Default for RocksDbArgs {
    fn default() -> Self {
        Self {
            all: false,
            tx_hash: default_tx_hash_in_rocksdb(),
            storages_history: default_storages_history_in_rocksdb(),
            account_history: default_account_history_in_rocksdb(),
            account_changesets: default_account_changesets_in_rocksdb(),
            storage_changesets: default_storage_changesets_in_rocksdb(),
        }
    }
}

impl RocksDbArgs {
    /// Validates the `RocksDB` arguments.
    ///
    /// Returns an error if `--rocksdb.all` is used with any individual flag set to `false`.
    pub const fn validate(&self) -> Result<(), RocksDbArgsError> {
        if self.all {
            if !self.tx_hash {
                return Err(RocksDbArgsError::ConflictingFlags("tx-hash"));
            }
            if !self.storages_history {
                return Err(RocksDbArgsError::ConflictingFlags("storages-history"));
            }
            if !self.account_history {
                return Err(RocksDbArgsError::ConflictingFlags("account-history"));
            }
            if !self.account_changesets {
                return Err(RocksDbArgsError::ConflictingFlags("account-changesets"));
            }
            if !self.storage_changesets {
                return Err(RocksDbArgsError::ConflictingFlags("storage-changesets"));
            }
        }
        Ok(())
    }
}

/// Error type for `RocksDB` argument validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RocksDbArgsError {
    /// `--rocksdb.all` cannot be combined with an individual flag set to false.
    #[error("--rocksdb.all cannot be combined with --rocksdb.{0}=false")]
    ConflictingFlags(&'static str),
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
    fn test_parse_all_flag() {
        let args = CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.all"]).args;
        assert!(args.all);
        assert_eq!(args.tx_hash, default_tx_hash_in_rocksdb());
    }

    #[test]
    fn test_defaults_match_storage_settings() {
        let args = RocksDbArgs::default();
        let settings = StorageSettings::base();
        assert_eq!(
            args.tx_hash, settings.transaction_hash_numbers_in_rocksdb,
            "tx_hash default should match StorageSettings::base()"
        );
        assert_eq!(
            args.storages_history, settings.storages_history_in_rocksdb,
            "storages_history default should match StorageSettings::base()"
        );
        assert_eq!(
            args.account_history, settings.account_history_in_rocksdb,
            "account_history default should match StorageSettings::base()"
        );
        assert_eq!(
            args.account_changesets, settings.account_changesets_in_rocksdb,
            "account_changesets default should match StorageSettings::base()"
        );
        assert_eq!(
            args.storage_changesets, settings.storage_changesets_in_rocksdb,
            "storage_changesets default should match StorageSettings::base()"
        );
    }

    #[test]
    fn test_parse_individual_flags() {
        let args = CommandParser::<RocksDbArgs>::parse_from([
            "reth",
            "--rocksdb.tx-hash=true",
            "--rocksdb.storages-history=false",
            "--rocksdb.account-history=true",
        ])
        .args;
        assert!(!args.all);
        assert!(args.tx_hash);
        assert!(!args.storages_history);
        assert!(args.account_history);
    }

    #[test]
    fn test_validate_all_with_true_ok() {
        let args = RocksDbArgs {
            all: true,
            tx_hash: true,
            storages_history: true,
            account_history: true,
            account_changesets: true,
            storage_changesets: true,
        };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_false_errors() {
        let base = RocksDbArgs {
            all: true,
            tx_hash: true,
            storages_history: true,
            account_history: true,
            account_changesets: true,
            storage_changesets: true,
        };

        let args = RocksDbArgs { tx_hash: false, ..base };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("tx-hash")));

        let args = RocksDbArgs { storages_history: false, ..base };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("storages-history")));

        let args = RocksDbArgs { account_history: false, ..base };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("account-history")));

        let args = RocksDbArgs { account_changesets: false, ..base };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("account-changesets")));

        let args = RocksDbArgs { storage_changesets: false, ..base };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("storage-changesets")));
    }
}
