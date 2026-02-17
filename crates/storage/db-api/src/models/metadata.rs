//! Storage metadata models.

use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// The storage layout format for trie nibble encoding.
///
/// Determines how nibble keys are encoded in the database trie tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageLayout {
    /// Legacy (v1) layout: 1 nibble per byte, 65-byte subkeys.
    V1,
    /// Packed (v2) layout: 2 nibbles per byte, 33-byte subkeys.
    V2,
}

impl StorageLayout {
    /// Returns `true` if this is the v2 (packed) layout.
    pub const fn is_v2(self) -> bool {
        matches!(self, Self::V2)
    }
}

/// Storage configuration settings for this node.
///
/// Controls whether this node uses v2 storage layout (static files + `RocksDB` routing)
/// or v1/legacy layout (everything in MDBX).
///
/// These should be set during `init_genesis` or `init_db` depending on whether we want dictate
/// behaviour of new or old nodes respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Compact, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StorageSettings {
    /// Whether this node uses v2 storage layout.
    ///
    /// When `true`, enables all v2 storage features:
    /// - Receipts and transaction senders in static files
    /// - History indices in `RocksDB` (accounts, storages, transaction hashes)
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

    /// Returns the storage layout for this node.
    pub const fn layout(&self) -> StorageLayout {
        if self.storage_v2 {
            StorageLayout::V2
        } else {
            StorageLayout::V1
        }
    }

    /// Whether receipts are stored in static files.
    pub const fn receipts_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether transaction senders are stored in static files.
    pub const fn transaction_senders_in_static_files(&self) -> bool {
        self.storage_v2
    }

    /// Whether storages history is stored in `RocksDB`.
    pub const fn storages_history_in_rocksdb(&self) -> bool {
        self.storage_v2
    }

    /// Whether transaction hash numbers are stored in `RocksDB`.
    pub const fn transaction_hash_numbers_in_rocksdb(&self) -> bool {
        self.storage_v2
    }

    /// Whether account history is stored in `RocksDB`.
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
