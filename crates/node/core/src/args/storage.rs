//! clap [Args](clap::Args) for storage configuration

use clap::Args;
use reth_storage_api::StorageSettings;

/// Parameters for storage configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Use hashed state tables (`HashedAccounts`/`HashedStorages`) as the canonical state
    /// representation instead of plain state tables (`PlainAccountState`/`PlainStorageState`).
    ///
    /// When enabled:
    /// - Execution writes directly to hashed tables, eliminating need for separate hashing stages
    /// - State reads come from hashed tables
    /// - `AccountHashingStage` and `StorageHashingStage` become no-ops
    ///
    /// WARNING: This setting is only configurable at database creation; changing it later
    /// requires re-syncing.
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
        base.use_hashed_state = self.use_hashed_state;

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
    #[cfg(feature = "edge")]
    pub fn storage_settings(&self) -> StorageSettings {
        self.apply_to(StorageSettings::edge())
    }

    /// Returns the default storage settings for new nodes, with CLI overrides applied.
    #[cfg(not(feature = "edge"))]
    pub fn storage_settings(&self) -> StorageSettings {
        self.apply_to(StorageSettings::legacy())
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
        assert!(!args.use_hashed_state);
    }

    #[test]
    fn test_use_hashed_state_flag() {
        let args =
            CommandParser::<StorageArgs>::parse_from(["reth", "--storage.use-hashed-state"]).args;
        assert!(args.use_hashed_state);
    }

    #[test]
    fn test_storage_settings_override() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.use-hashed-state",
            "--storage.receipts-in-static-files",
            "true",
        ])
        .args;

        let base = StorageSettings::legacy();
        let settings = args.apply_to(base);

        assert!(settings.use_hashed_state);
        assert!(settings.receipts_in_static_files);
        assert!(!settings.transaction_senders_in_static_files);
    }

    #[test]
    fn test_all_storage_flags() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.use-hashed-state",
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

        assert!(settings.use_hashed_state);
        assert!(settings.receipts_in_static_files);
        assert!(settings.transaction_senders_in_static_files);
        assert!(settings.account_changesets_in_static_files);
        assert!(settings.storages_history_in_rocksdb);
        assert!(settings.transaction_hash_numbers_in_rocksdb);
        assert!(settings.account_history_in_rocksdb);
    }
}
