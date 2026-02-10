//! Storage metadata models.

use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// Storage configuration settings for this node.
///
/// Controls whether this node uses v2 storage layout (static files + RocksDB routing)
/// or v1/legacy layout (everything in MDBX).
///
/// These should be set during `init_genesis` or `init_db` depending on whether we want dictate
/// behaviour of new or old nodes respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StorageSettings {
    /// Whether this node uses v2 storage layout.
    ///
    /// When `true`, enables all v2 storage features:
    /// - Receipts and transaction senders in static files
    /// - History indices in RocksDB (accounts, storages, transaction hashes)
    /// - Account and storage changesets in static files
    /// - Hashed state tables as canonical state representation
    ///
    /// When `false`, uses v1/legacy layout (everything in MDBX).
    pub storage_v2: bool,
}

impl StorageSettings {
    /// Returns the default base `StorageSettings`.
    ///
    /// When the `edge` feature is enabled, returns [`Self::v2()`] so that CI and
    /// edge builds automatically use v2 storage defaults. Otherwise returns
    /// [`Self::v1()`]. The `--storage.v2` CLI flag can also opt into v2 at runtime
    /// regardless of feature flags.
    pub const fn base() -> Self {
        #[cfg(feature = "edge")]
        {
            Self::v2()
        }
        #[cfg(not(feature = "edge"))]
        {
            Self::v1()
        }
    }

    /// Creates `StorageSettings` for v2 nodes with all storage features enabled:
    /// - Receipts and transaction senders in static files
    /// - History indices in `RocksDB` (storages, accounts, transaction hashes)
    /// - Account and storage changesets in static files
    /// - Hashed state as canonical state representation
    ///
    /// Use this when the `--storage.v2` CLI flag is set.
    pub const fn v2() -> Self {
        Self { storage_v2: true }
    }

    /// Creates `StorageSettings` for v1/legacy nodes.
    ///
    /// This keeps all data in MDBX, matching the original storage layout.
    pub const fn v1() -> Self {
        Self { storage_v2: false }
    }

    /// Returns `true` if this node uses v2 storage layout.
    pub const fn is_v2(&self) -> bool {
        self.storage_v2
    }

    /// Whether receipts are stored in static files.
    pub const fn receipts_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether transaction senders are stored in static files.
    pub const fn transaction_senders_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether account changesets are stored in static files.
    pub const fn account_changesets_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether storage changesets are stored in static files.
    pub const fn storage_changesets_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether storages history is stored in RocksDB.
    pub const fn storages_history_in_rocksdb(&self) -> bool {
        self.storage_v2
    }

    /// Whether transaction hash numbers are stored in RocksDB.
    pub const fn transaction_hash_numbers_in_rocksdb(&self) -> bool {
        self.storage_v2
    }

    /// Whether account history is stored in RocksDB.
    pub const fn account_history_in_rocksdb(&self) -> bool {
        self.storage_v2
    }

    /// Whether to use hashed state tables (`HashedAccounts`/`HashedStorages`) as the canonical
    /// state representation instead of plain state tables. Implied by v2 storage layout.
    pub const fn use_hashed_state(&self) -> bool {
        self.storage_v2
    }

    /// Returns `true` if any tables are configured to be stored in `RocksDB`.
    pub const fn any_in_rocksdb(&self) -> bool {
        self.storage_v2
    }
}

/// Custom serialization: always writes the new shape.
impl Serialize for StorageSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("StorageSettings", 1)?;
        s.serialize_field("storage_v2", &self.storage_v2)?;
        s.end()
    }
}

/// Custom deserialization: accepts both the new shape (`storage_v2`)
/// and the legacy shape with 8 individual booleans.
///
/// Legacy settings are mapped conservatively: `storage_v2 = true` only if all 7 routing
/// booleans were `true` (matching `StorageSettings::v2()`). Otherwise `storage_v2 = false`.
impl<'de> Deserialize<'de> for StorageSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Legacy {
            #[serde(default)]
            receipts_in_static_files: bool,
            #[serde(default)]
            transaction_senders_in_static_files: bool,
            #[serde(default)]
            storages_history_in_rocksdb: bool,
            #[serde(default)]
            transaction_hash_numbers_in_rocksdb: bool,
            #[serde(default)]
            account_history_in_rocksdb: bool,
            #[serde(default)]
            account_changesets_in_static_files: bool,
            #[serde(default)]
            storage_changesets_in_static_files: bool,
        }

        let value = serde_json::Value::deserialize(deserializer)?;

        if value.get("storage_v2").is_some() {
            #[derive(Deserialize)]
            struct New {
                #[serde(default)]
                storage_v2: bool,
            }
            let new: New = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self { storage_v2: new.storage_v2 })
        } else {
            let legacy: Legacy = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            let is_v2 = legacy.receipts_in_static_files &&
                legacy.transaction_senders_in_static_files &&
                legacy.storages_history_in_rocksdb &&
                legacy.transaction_hash_numbers_in_rocksdb &&
                legacy.account_history_in_rocksdb &&
                legacy.account_changesets_in_static_files &&
                legacy.storage_changesets_in_static_files;
            Ok(Self { storage_v2: is_v2 })
        }
    }
}
