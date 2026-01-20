//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};

/// Parameters for `RocksDB` table routing configuration.
///
/// These flags control which database tables are stored in `RocksDB` instead of MDBX.
/// All flags are genesis-initialization-only: changing them after genesis requires a re-sync.
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "RocksDB")]
pub struct RocksDbArgs {
    /// Route all supported tables to `RocksDB` instead of MDBX.
    ///
    /// This enables `RocksDB` for `tx-hash`, `storages-history`, and `account-history` tables.
    /// Cannot be combined with individual flags set to false.
    #[arg(long = "rocksdb.all", action = ArgAction::SetTrue)]
    pub all: bool,

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
    /// Validates the `RocksDB` arguments.
    ///
    /// Returns an error if `--rocksdb.all` is used with any individual flag set to `false`.
    pub fn validate(&self) -> Result<(), RocksDbArgsError> {
        if self.all {
            if self.tx_hash == Some(false) {
                return Err(RocksDbArgsError::ConflictingFlags("tx-hash"));
            }
            if self.storages_history == Some(false) {
                return Err(RocksDbArgsError::ConflictingFlags("storages-history"));
            }
            if self.account_history == Some(false) {
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
    }

    #[test]
    fn test_parse_all_flag() {
        let args = CommandParser::<RocksDbArgs>::parse_from(["reth", "--rocksdb.all"]).args;
        assert!(args.all);
        assert_eq!(args.tx_hash, None);
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
    fn test_validate_all_alone_ok() {
        let args = RocksDbArgs { all: true, ..Default::default() };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_true_ok() {
        let args = RocksDbArgs { all: true, tx_hash: Some(true), ..Default::default() };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_false_errors() {
        let args = RocksDbArgs { all: true, tx_hash: Some(false), ..Default::default() };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("tx-hash")));

        let args = RocksDbArgs { all: true, storages_history: Some(false), ..Default::default() };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("storages-history")));

        let args = RocksDbArgs { all: true, account_history: Some(false), ..Default::default() };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("account-history")));
    }
}
