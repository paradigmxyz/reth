//! clap [Args](clap::Args) for `RocksDB` table routing configuration

use clap::{ArgAction, Args};

/// Default value for `RocksDB` routing flags.
///
/// When the `edge` feature is enabled, defaults to `true` to enable edge storage features.
/// Otherwise defaults to `false` for legacy behavior.
const fn default_rocksdb_flag() -> bool {
    cfg!(feature = "edge")
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
    #[arg(long = "rocksdb.tx-hash", default_value_t = default_rocksdb_flag(), action = ArgAction::Set)]
    pub tx_hash: bool,

    /// Route storages history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `true` when the `edge` feature is enabled, `false` otherwise.
    #[arg(long = "rocksdb.storages-history", default_value_t = default_rocksdb_flag(), action = ArgAction::Set)]
    pub storages_history: bool,

    /// Route account history tables to `RocksDB` instead of MDBX.
    ///
    /// This is a genesis-initialization-only flag: changing it after genesis requires a re-sync.
    /// Defaults to `true` when the `edge` feature is enabled, `false` otherwise.
    #[arg(long = "rocksdb.account-history", default_value_t = default_rocksdb_flag(), action = ArgAction::Set)]
    pub account_history: bool,
}

impl Default for RocksDbArgs {
    fn default() -> Self {
        Self {
            all: false,
            tx_hash: default_rocksdb_flag(),
            storages_history: default_rocksdb_flag(),
            account_history: default_rocksdb_flag(),
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
        assert_eq!(args.tx_hash, default_rocksdb_flag());
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
        let args =
            RocksDbArgs { all: true, tx_hash: true, storages_history: true, account_history: true };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_validate_all_with_false_errors() {
        let args = RocksDbArgs {
            all: true,
            tx_hash: false,
            storages_history: true,
            account_history: true,
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("tx-hash")));

        let args = RocksDbArgs {
            all: true,
            tx_hash: true,
            storages_history: false,
            account_history: true,
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("storages-history")));

        let args = RocksDbArgs {
            all: true,
            tx_hash: true,
            storages_history: true,
            account_history: false,
        };
        assert_eq!(args.validate(), Err(RocksDbArgsError::ConflictingFlags("account-history")));
    }
}
