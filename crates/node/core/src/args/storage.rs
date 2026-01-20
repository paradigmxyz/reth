//! clap [Args](clap::Args) for storage configuration

use clap::{ArgAction, Args};
use reth_storage_api::StorageSettings;

/// Parameters for storage configuration.
///
/// This controls whether the node uses v2 storage defaults (with `RocksDB` and static file
/// optimizations) or v1/legacy storage defaults, plus additional storage settings.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Enable v2 storage defaults (static files + `RocksDB` routing).
    ///
    /// When enabled, the node uses optimized storage settings:
    /// - Receipts and transaction senders in static files
    /// - History indices in `RocksDB` (accounts, storages, transaction hashes)
    /// - Account and storage changesets in static files
    ///
    /// This is a genesis-initialization-only setting: changing it after genesis requires a
    /// re-sync.
    ///
    /// Individual settings can still be overridden with `--static-files.*` and `--rocksdb.*`
    /// flags.
    #[arg(long = "storage.v2", action = ArgAction::SetTrue)]
    pub v2: bool,

    /// Use hashed state tables (`HashedAccounts`/`HashedStorages`) as canonical state
    /// representation instead of plain state tables.
    ///
    /// When enabled, execution writes directly to hashed tables, eliminating the need for
    /// separate hashing stages. This should only be enabled for new databases.
    ///
    /// WARNING: Changing this setting on an existing database requires a full resync.
    #[arg(long = "storage.use-hashed-state", default_value_t = false)]
    pub use_hashed_state: bool,

    /// Store receipts in static files instead of the database.
    ///
    /// This is enabled by default for new nodes. Existing nodes continue using their
    /// current storage location unless migrated.
    #[arg(long = "storage.receipts-in-static-files")]
    pub receipts_in_static_files: Option<bool>,

    /// Store transaction senders in static files instead of the database.
    #[arg(long = "storage.transaction-senders-in-static-files")]
    pub transaction_senders_in_static_files: Option<bool>,

    /// Store account changesets in static files instead of the database.
    #[arg(long = "storage.account-changesets-in-static-files")]
    pub account_changesets_in_static_files: Option<bool>,

    /// Store storage history in RocksDB instead of MDBX.
    #[arg(long = "storage.storages-history-in-rocksdb")]
    pub storages_history_in_rocksdb: Option<bool>,

    /// Store transaction hash to number mapping in RocksDB instead of MDBX.
    #[arg(long = "storage.transaction-hash-numbers-in-rocksdb")]
    pub transaction_hash_numbers_in_rocksdb: Option<bool>,

    /// Store account history in RocksDB instead of MDBX.
    #[arg(long = "storage.account-history-in-rocksdb")]
    pub account_history_in_rocksdb: Option<bool>,
}

impl StorageArgs {
    /// Applies CLI overrides to the given base settings.
    ///
    /// Fields that are `None` in the CLI args will use the base settings value.
    /// Fields that are `Some` will override the base settings.
    pub fn apply_to(&self, mut base: StorageSettings) -> StorageSettings {
        if let Some(v) = self.receipts_in_static_files {
            base.receipts_in_static_files = v;
        }
        if let Some(v) = self.transaction_senders_in_static_files {
            base.transaction_senders_in_static_files = v;
        }
        if let Some(v) = self.account_changesets_in_static_files {
            base.account_changesets_in_static_files = v;
        }
        if let Some(v) = self.storages_history_in_rocksdb {
            base.storages_history_in_rocksdb = v;
        }
        if let Some(v) = self.transaction_hash_numbers_in_rocksdb {
            base.transaction_hash_numbers_in_rocksdb = v;
        }
        if let Some(v) = self.account_history_in_rocksdb {
            base.account_history_in_rocksdb = v;
        }

        base
    }

    /// Returns the default storage settings for new nodes, with CLI overrides applied.
    ///
    /// Uses v2 settings if `--storage.v2` is set, otherwise uses v1 settings.
    pub fn storage_settings(&self) -> StorageSettings {
        let base = if self.v2 { StorageSettings::v2() } else { StorageSettings::v1() };
        self.apply_to(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_default_storage_args() {
        let default_args = StorageArgs::default();
        let args = CommandParser::<StorageArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
        assert!(!args.v2);
    }

    #[test]
    fn test_parse_v2_flag() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2"]).args;
        assert!(args.v2);
    }

    #[test]
    fn test_storage_settings_override() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.receipts-in-static-files",
            "true",
        ])
        .args;

        let settings = args.storage_settings();

        assert!(settings.receipts_in_static_files);
        assert!(!settings.transaction_senders_in_static_files);
    }

    #[test]
    fn test_all_storage_flags() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.receipts-in-static-files",
            "true",
            "--storage.transaction-senders-in-static-files",
            "true",
            "--storage.account-changesets-in-static-files",
            "true",
            "--storage.storages-history-in-rocksdb",
            "true",
            "--storage.transaction-hash-numbers-in-rocksdb",
            "true",
            "--storage.account-history-in-rocksdb",
            "true",
        ])
        .args;

        let settings = args.apply_to(StorageSettings::default());

        assert!(settings.receipts_in_static_files);
        assert!(settings.transaction_senders_in_static_files);
        assert!(settings.account_changesets_in_static_files);
        assert!(settings.storages_history_in_rocksdb);
        assert!(settings.transaction_hash_numbers_in_rocksdb);
        assert!(settings.account_history_in_rocksdb);
    }

    #[test]
    fn test_v2_storage_settings() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2"]).args;
        let settings = args.storage_settings();

        assert!(settings.receipts_in_static_files);
        assert!(settings.transaction_senders_in_static_files);
        assert!(settings.account_changesets_in_static_files);
        assert!(settings.storage_changesets_in_static_files);
        assert!(settings.storages_history_in_rocksdb);
        assert!(settings.transaction_hash_numbers_in_rocksdb);
        assert!(settings.account_history_in_rocksdb);
    }
}
