//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};

/// Parameters for `RocksDB` table routing configuration.
///
/// These flags control which database tables are stored in `RocksDB` instead of MDBX.
/// All flags are genesis-initialization-only: changing them after genesis requires a re-sync.
///
/// When `--storage.v2` is used, the defaults for these flags change to enable RocksDB routing.
/// Individual flags can still override those defaults when explicitly set.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
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
    /// Defaults to the base storage mode (v1: false, v2: true).
    #[arg(long = "rocksdb.tx-hash", action = ArgAction::Set)]
    pub tx_hash: Option<bool>,

    /// Route storages history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to the base storage mode (v1: false, v2: true).
    #[arg(long = "rocksdb.storages-history", action = ArgAction::Set)]
    pub storages_history: Option<bool>,

    /// Route account history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to the base storage mode (v1: false, v2: true).
    #[arg(long = "rocksdb.account-history", action = ArgAction::Set)]
    pub account_history: Option<bool>,
}

impl RocksDbArgs {
    /// Validates the `RocksDB` arguments.
    ///
    /// Returns an error if `--rocksdb.all` is used with any individual flag explicitly set to
    /// `false`.
    pub const fn validate(&self) -> Result<(), RocksDbArgsError> {
        if self.all {
            if matches!(self.tx_hash, Some(false)) {
                return Err(RocksDbArgsError::ConflictingFlags("tx-hash"));
            }
            if matches!(self.storages_history, Some(false)) {
                return Err(RocksDbArgsError::ConflictingFlags("storages-history"));
            }
            if matches!(self.account_history, Some(false)) {
                return Err(RocksDbArgsError::ConflictingFlags("account-history"));
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
        assert!(!args.all);
        assert!(args.tx_hash.is_none());
        assert!(args.storages_history.is_none());
        assert!(args.account_history.is_none());
    }

    #[test]
    fn test_parse_all_flag() {
        let args = CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.all"]).args;
        assert!(args.all);
        assert!(args.tx_hash.is_none());
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
        assert_eq!(args.tx_hash, Some(true));
        assert_eq!(args.storages_history, Some(false));
        assert_eq!(args.account_history, Some(true));
    }

    #[test]
    fn test_validate_all_with_none_ok() {
        let args =
            RocksDbArgs { all: true, tx_hash: None, storages_history: None, account_history: None };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_true_ok() {
        let args = RocksDbArgs {
            all: true,
            tx_hash: Some(true),
            storages_history: Some(true),
            account_history: Some(true),
        };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_false_errors() {
        let args = RocksDbArgs {
            all: true,
            tx_hash: Some(false),
            storages_history: None,
            account_history: None,
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("tx-hash")));

        let args = RocksDbArgs {
            all: true,
            tx_hash: None,
            storages_history: Some(false),
            account_history: None,
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("storages-history")));

        let args = RocksDbArgs {
            all: true,
            tx_hash: None,
            storages_history: None,
            account_history: Some(false),
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("account-history")));
    }
}
