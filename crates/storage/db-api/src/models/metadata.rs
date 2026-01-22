//! Storage metadata models.

use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// Storage configuration settings for this node.
///
/// These should be set during `init_genesis` or `init_db` depending on whether we want dictate
/// behaviour of new or old nodes respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StorageSettings {
    /// Whether this node always writes receipts to static files.
    ///
    /// If this is set to FALSE AND receipt pruning IS ENABLED, all receipts should be written to DB. Otherwise, they should be written to static files. This ensures that older nodes do not need to migrate their current DB tables to static files. For more, read: <https://github.com/paradigmxyz/reth/issues/18890#issuecomment-3457760097>
    #[serde(default)]
    pub receipts_in_static_files: bool,
    /// Whether this node always writes transaction senders to static files.
    #[serde(default)]
    pub transaction_senders_in_static_files: bool,
    /// Whether `StoragesHistory` is stored in `RocksDB`.
    #[serde(default)]
    pub storages_history_in_rocksdb: bool,
    /// Whether `TransactionHashNumbers` is stored in `RocksDB`.
    #[serde(default)]
    pub transaction_hash_numbers_in_rocksdb: bool,
    /// Whether `AccountsHistory` is stored in `RocksDB`.
    #[serde(default)]
    pub account_history_in_rocksdb: bool,
    /// Whether this node should read and write account changesets from static files.
    #[serde(default)]
    pub account_changesets_in_static_files: bool,
}

impl StorageSettings {
    /// Returns the default base `StorageSettings` for this build.
    ///
    /// When the `edge` feature is enabled, returns [`Self::edge()`].
    /// Otherwise, returns [`Self::legacy()`].
    pub const fn base() -> Self {
        #[cfg(feature = "edge")]
        {
            Self::edge()
        }
        #[cfg(not(feature = "edge"))]
        {
            Self::legacy()
        }
    }

    /// Creates `StorageSettings` for edge nodes with all storage features enabled:
    /// - Receipts and transaction senders in static files
    /// - History indices in `RocksDB` (storages, accounts, transaction hashes)
    /// - Account changesets in static files
    #[cfg(feature = "edge")]
    pub const fn edge() -> Self {
        Self {
            receipts_in_static_files: true,
            transaction_senders_in_static_files: true,
            account_changesets_in_static_files: true,
            storages_history_in_rocksdb: false,
            transaction_hash_numbers_in_rocksdb: true,
            account_history_in_rocksdb: false,
        }
    }

    /// Creates `StorageSettings` for legacy nodes.
    ///
    /// This explicitly sets `receipts_in_static_files` and `transaction_senders_in_static_files` to
    /// `false`, ensuring older nodes continue writing receipts and transaction senders to the
    /// database when receipt pruning is enabled.
    pub const fn legacy() -> Self {
        Self {
            receipts_in_static_files: false,
            transaction_senders_in_static_files: false,
            storages_history_in_rocksdb: false,
            transaction_hash_numbers_in_rocksdb: false,
            account_history_in_rocksdb: false,
            account_changesets_in_static_files: false,
        }
    }

    /// Sets the `receipts_in_static_files` flag to the provided value.
    pub const fn with_receipts_in_static_files(mut self, value: bool) -> Self {
        self.receipts_in_static_files = value;
        self
    }

    /// Sets the `transaction_senders_in_static_files` flag to the provided value.
    pub const fn with_transaction_senders_in_static_files(mut self, value: bool) -> Self {
        self.transaction_senders_in_static_files = value;
        self
    }

    /// Sets the `storages_history_in_rocksdb` flag to the provided value.
    pub const fn with_storages_history_in_rocksdb(mut self, value: bool) -> Self {
        self.storages_history_in_rocksdb = value;
        self
    }

    /// Sets the `transaction_hash_numbers_in_rocksdb` flag to the provided value.
    pub const fn with_transaction_hash_numbers_in_rocksdb(mut self, value: bool) -> Self {
        self.transaction_hash_numbers_in_rocksdb = value;
        self
    }

    /// Sets the `account_history_in_rocksdb` flag to the provided value.
    pub const fn with_account_history_in_rocksdb(mut self, value: bool) -> Self {
        self.account_history_in_rocksdb = value;
        self
    }

    /// Sets the `account_changesets_in_static_files` flag to the provided value.
    pub const fn with_account_changesets_in_static_files(mut self, value: bool) -> Self {
        self.account_changesets_in_static_files = value;
        self
    }

    /// Returns `true` if any tables are configured to be stored in `RocksDB`.
    pub const fn any_in_rocksdb(&self) -> bool {
        self.transaction_hash_numbers_in_rocksdb ||
            self.account_history_in_rocksdb ||
            self.storages_history_in_rocksdb
    }

    /// Returns `true` if any history indices are configured for `RocksDB`.
    pub const fn any_history_in_rocksdb(&self) -> bool {
        self.account_history_in_rocksdb || self.storages_history_in_rocksdb
    }

    /// Validates that the storage settings are consistent.
    ///
    /// # Invariant
    ///
    /// `RocksDB` history indices (`AccountsHistory`, `StoragesHistory`) are derived data
    /// that index into changesets. They can only be safely stored in `RocksDB` when
    /// changesets are in static files (append-only, crash-stable), not MDBX (can be unwound).
    ///
    /// This is because after a crash during unwind, `RocksDB` history could reference
    /// block numbers whose changesets were already removed from MDBX, causing "future
    /// state lookups" or missing data.
    ///
    /// Returns `Err` with a description if the configuration is invalid.
    pub const fn validate(&self) -> Result<(), &'static str> {
        if self.any_history_in_rocksdb() && !self.account_changesets_in_static_files {
            return Err("RocksDB history indices require account_changesets_in_static_files=true; \
                 otherwise crash/unwind can diverge RocksDB history from MDBX changesets");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_settings_validate_legacy_is_valid() {
        assert!(StorageSettings::legacy().validate().is_ok());
    }

    #[test]
    #[cfg(feature = "edge")]
    fn test_storage_settings_validate_edge_is_valid() {
        assert!(StorageSettings::edge().validate().is_ok());
    }

    #[test]
    fn test_storage_settings_validate_history_without_changesets_is_invalid() {
        let settings = StorageSettings::legacy().with_account_history_in_rocksdb(true);
        assert!(settings.validate().is_err());

        let settings = StorageSettings::legacy().with_storages_history_in_rocksdb(true);
        assert!(settings.validate().is_err());
    }

    #[test]
    fn test_storage_settings_validate_history_with_changesets_is_valid() {
        let settings = StorageSettings::legacy()
            .with_account_changesets_in_static_files(true)
            .with_account_history_in_rocksdb(true);
        assert!(settings.validate().is_ok());

        let settings = StorageSettings::legacy()
            .with_account_changesets_in_static_files(true)
            .with_storages_history_in_rocksdb(true);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn test_storage_settings_validate_tx_hash_without_changesets_is_valid() {
        let settings = StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true);
        assert!(settings.validate().is_ok());
    }
}
