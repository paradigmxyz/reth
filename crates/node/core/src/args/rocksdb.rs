//! clap [Args](clap::Args) for `RocksDB` storage configuration

use clap::Args;
use reth_provider::StorageSettings;

/// Parameters for `RocksDB` storage configuration.
///
/// These flags control which tables are stored in `RocksDB` instead of MDBX.
/// `RocksDB` provides better write performance for append-heavy workloads like
/// history indices.
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "RocksDB Storage")]
pub struct RocksDBArgs {
    /// Store account history indices in `RocksDB` instead of MDBX.
    ///
    /// When enabled, the `AccountsHistory` table will be stored in `RocksDB`,
    /// which provides better write performance for this append-heavy workload.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "rocksdb.account-history")]
    pub account_history: bool,

    /// Store storage history indices in `RocksDB` instead of MDBX.
    ///
    /// When enabled, the `StoragesHistory` table will be stored in `RocksDB`,
    /// which provides better write performance for this append-heavy workload.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "rocksdb.storages-history")]
    pub storages_history: bool,

    /// Store transaction hash to number mappings in `RocksDB` instead of MDBX.
    ///
    /// When enabled, the `TransactionHashNumbers` table will be stored in `RocksDB`,
    /// which provides better write performance for this append-heavy workload.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "rocksdb.tx-hash-numbers")]
    pub tx_hash_numbers: bool,
}

impl RocksDBArgs {
    /// Applies `RocksDB` settings to an existing [`StorageSettings`].
    pub const fn apply_to_settings(&self, settings: StorageSettings) -> StorageSettings {
        settings
            .with_account_history_in_rocksdb(self.account_history)
            .with_storages_history_in_rocksdb(self.storages_history)
            .with_transaction_hash_numbers_in_rocksdb(self.tx_hash_numbers)
    }

    /// Returns true if any `RocksDB` storage option is enabled.
    pub const fn any_enabled(&self) -> bool {
        self.account_history || self.storages_history || self.tx_hash_numbers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_args() {
        let args = RocksDBArgs::default();
        assert!(!args.account_history);
        assert!(!args.storages_history);
        assert!(!args.tx_hash_numbers);
        assert!(!args.any_enabled());
    }

    #[test]
    fn test_apply_to_settings() {
        let args =
            RocksDBArgs { account_history: true, storages_history: true, tx_hash_numbers: false };
        let settings = args.apply_to_settings(StorageSettings::legacy());

        assert!(settings.account_history_in_rocksdb);
        assert!(settings.storages_history_in_rocksdb);
        assert!(!settings.transaction_hash_numbers_in_rocksdb);
    }

    #[test]
    fn test_any_enabled() {
        let args = RocksDBArgs { account_history: true, ..Default::default() };
        assert!(args.any_enabled());

        let args = RocksDBArgs { storages_history: true, ..Default::default() };
        assert!(args.any_enabled());

        let args = RocksDBArgs { tx_hash_numbers: true, ..Default::default() };
        assert!(args.any_enabled());
    }
}
