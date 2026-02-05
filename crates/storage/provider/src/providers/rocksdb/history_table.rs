//! Unified traits for history table operations in `RocksDB`.
//!
//! This module provides `HistoryTable` and `HistorySpec` traits that abstract
//! the differences between `AccountsHistory` and `StoragesHistory` tables,
//! enabling unified code for history shard operations.

use alloy_primitives::{Address, BlockNumber, B256};
use reth_db_api::{
    models::{
        sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey, ShardedKey,
        StorageSettings,
    },
    table::Table,
    tables, BlockNumberList,
};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;

/// Trait unifying `RocksDB` history shard behavior for `AccountsHistory` and `StoragesHistory`.
pub trait HistoryTable: 'static + Send + Sync {
    /// The `RocksDB` table (column family).
    type Table: Table<Value = BlockNumberList>;

    /// The logical entity whose history we store.
    /// - Accounts: `Address`
    /// - Storage: `(Address, B256)` (address + slot)
    type PartialKey: Clone + core::fmt::Debug + Eq + core::hash::Hash + Send + Sync;

    /// Concrete shard key type used by the table.
    /// - `AccountsHistory`: `ShardedKey<Address>`
    /// - `StoragesHistory`: `StorageShardedKey`
    type ShardKey: Clone + core::fmt::Debug + Ord + Send + Sync;

    /// How many block indices fit in a shard for this history type.
    const INDICES_PER_SHARD: u64;

    /// Sentinel used for the "last shard" key (current code uses `u64::MAX`).
    const LAST_SHARD_SENTINEL: BlockNumber = u64::MAX;

    /// Create a shard key from partial key + highest block number boundary.
    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey;

    /// Extract partial key from a shard key.
    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey;

    /// Extract highest block boundary from shard key.
    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber;

    /// Return the smallest key for iterating all shards for `partial`.
    fn first_shard_key(partial: Self::PartialKey) -> Self::ShardKey {
        Self::make_shard_key(partial, 0)
    }

    /// Whether `key` belongs to the same partial key.
    fn is_same_partial(key: &Self::ShardKey, partial: &Self::PartialKey) -> bool {
        Self::partial_from_shard_key(key) == *partial
    }
}

/// Marker for `AccountsHistory` table operations.
#[derive(Debug, Clone, Copy)]
pub struct AccountsHistoryTable;

/// Marker for `StoragesHistory` table operations.
#[derive(Debug, Clone, Copy)]
pub struct StoragesHistoryTable;

impl HistoryTable for AccountsHistoryTable {
    type Table = tables::AccountsHistory;
    type PartialKey = Address;
    type ShardKey = ShardedKey<Address>;

    const INDICES_PER_SHARD: u64 = NUM_OF_INDICES_IN_SHARD as u64;

    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey {
        ShardedKey::new(partial, highest)
    }

    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey {
        key.key
    }

    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber {
        key.highest_block_number
    }
}

impl HistoryTable for StoragesHistoryTable {
    type Table = tables::StoragesHistory;
    type PartialKey = (Address, B256);
    type ShardKey = StorageShardedKey;

    const INDICES_PER_SHARD: u64 = NUM_OF_INDICES_IN_SHARD as u64;

    fn make_shard_key(partial: Self::PartialKey, highest: BlockNumber) -> Self::ShardKey {
        StorageShardedKey::new(partial.0, partial.1, highest)
    }

    fn partial_from_shard_key(key: &Self::ShardKey) -> Self::PartialKey {
        (key.address, key.sharded_key.key)
    }

    fn highest_from_shard_key(key: &Self::ShardKey) -> BlockNumber {
        key.sharded_key.highest_block_number
    }
}

/// Trait for history pipeline metadata (stages, pruning, static files).
pub trait HistorySpec: 'static + Send + Sync {
    /// The underlying history table type.
    type HT: HistoryTable;

    /// Stage ID for indexing this history type.
    const STAGE_ID: StageId;

    /// Prune segment for this history type.
    const PRUNE_SEGMENT: PruneSegment;

    /// Static file segment for the changesets backing this history.
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment;

    /// Whether this history type is stored in `RocksDB` based on settings.
    fn history_in_rocksdb(settings: &StorageSettings) -> bool;

    /// Whether the changesets for this history are in static files.
    fn changesets_in_static_files(settings: &StorageSettings) -> bool;
}

/// `HistorySpec` implementation for account history.
#[derive(Debug, Clone, Copy)]
pub struct AccountHistorySpec;

/// `HistorySpec` implementation for storage history.
#[derive(Debug, Clone, Copy)]
pub struct StorageHistorySpec;

impl HistorySpec for AccountHistorySpec {
    type HT = AccountsHistoryTable;

    const STAGE_ID: StageId = StageId::IndexAccountHistory;
    const PRUNE_SEGMENT: PruneSegment = PruneSegment::AccountHistory;
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment = StaticFileSegment::AccountChangeSets;

    fn history_in_rocksdb(settings: &StorageSettings) -> bool {
        settings.account_history_in_rocksdb
    }

    fn changesets_in_static_files(settings: &StorageSettings) -> bool {
        settings.account_changesets_in_static_files
    }
}

impl HistorySpec for StorageHistorySpec {
    type HT = StoragesHistoryTable;

    const STAGE_ID: StageId = StageId::IndexStorageHistory;
    const PRUNE_SEGMENT: PruneSegment = PruneSegment::StorageHistory;
    const CHANGESET_STATIC_SEGMENT: StaticFileSegment = StaticFileSegment::StorageChangeSets;

    fn history_in_rocksdb(settings: &StorageSettings) -> bool {
        settings.storages_history_in_rocksdb
    }

    fn changesets_in_static_files(settings: &StorageSettings) -> bool {
        settings.storage_changesets_in_static_files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn account_history_shard_key_ordering() {
        let addr = address!("0102030405060708091011121314151617181920");

        let first = AccountsHistoryTable::first_shard_key(addr);
        let last =
            AccountsHistoryTable::make_shard_key(addr, AccountsHistoryTable::LAST_SHARD_SENTINEL);

        assert!(first < last, "first_shard_key should sort before last_shard_key");
        assert_eq!(AccountsHistoryTable::highest_from_shard_key(&first), 0);
        assert_eq!(AccountsHistoryTable::highest_from_shard_key(&last), u64::MAX);
    }

    #[test]
    fn storage_history_shard_key_ordering() {
        let addr = address!("0102030405060708091011121314151617181920");
        let slot = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let partial = (addr, slot);

        let first = StoragesHistoryTable::first_shard_key(partial.clone());
        let last = StoragesHistoryTable::make_shard_key(
            partial.clone(),
            StoragesHistoryTable::LAST_SHARD_SENTINEL,
        );

        assert!(first < last, "first_shard_key should sort before last_shard_key");
        assert_eq!(StoragesHistoryTable::highest_from_shard_key(&first), 0);
        assert_eq!(StoragesHistoryTable::highest_from_shard_key(&last), u64::MAX);
    }

    #[test]
    fn account_history_is_same_partial() {
        let addr1 = address!("0102030405060708091011121314151617181920");
        let addr2 = address!("aabbccddee112233445566778899aabbccddeeff");

        let key1 = AccountsHistoryTable::make_shard_key(addr1, 100);
        let key2 = AccountsHistoryTable::make_shard_key(addr1, 200);
        let key3 = AccountsHistoryTable::make_shard_key(addr2, 100);

        assert!(AccountsHistoryTable::is_same_partial(&key1, &addr1));
        assert!(AccountsHistoryTable::is_same_partial(&key2, &addr1));
        assert!(!AccountsHistoryTable::is_same_partial(&key3, &addr1));
        assert!(AccountsHistoryTable::is_same_partial(&key3, &addr2));
    }

    #[test]
    fn storage_history_is_same_partial() {
        let addr = address!("0102030405060708091011121314151617181920");
        let slot1 = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let slot2 = b256!("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
        let partial1 = (addr, slot1);
        let partial2 = (addr, slot2);

        let key1 = StoragesHistoryTable::make_shard_key(partial1.clone(), 100);
        let key2 = StoragesHistoryTable::make_shard_key(partial1.clone(), 200);
        let key3 = StoragesHistoryTable::make_shard_key(partial2.clone(), 100);

        assert!(StoragesHistoryTable::is_same_partial(&key1, &partial1));
        assert!(StoragesHistoryTable::is_same_partial(&key2, &partial1));
        assert!(!StoragesHistoryTable::is_same_partial(&key3, &partial1));
        assert!(StoragesHistoryTable::is_same_partial(&key3, &partial2));
    }

    #[test]
    fn account_history_roundtrip() {
        let addr = address!("0102030405060708091011121314151617181920");
        let block = 12345u64;

        let key = AccountsHistoryTable::make_shard_key(addr, block);
        assert_eq!(AccountsHistoryTable::partial_from_shard_key(&key), addr);
        assert_eq!(AccountsHistoryTable::highest_from_shard_key(&key), block);
    }

    #[test]
    fn storage_history_roundtrip() {
        let addr = address!("0102030405060708091011121314151617181920");
        let slot = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let block = 12345u64;
        let partial = (addr, slot);

        let key = StoragesHistoryTable::make_shard_key(partial.clone(), block);
        assert_eq!(StoragesHistoryTable::partial_from_shard_key(&key), partial);
        assert_eq!(StoragesHistoryTable::highest_from_shard_key(&key), block);
    }

    #[test]
    fn indices_per_shard_matches_constant() {
        assert_eq!(AccountsHistoryTable::INDICES_PER_SHARD, NUM_OF_INDICES_IN_SHARD as u64);
        assert_eq!(StoragesHistoryTable::INDICES_PER_SHARD, NUM_OF_INDICES_IN_SHARD as u64);
    }

    #[test]
    fn history_spec_constants() {
        assert_eq!(AccountHistorySpec::STAGE_ID, StageId::IndexAccountHistory);
        assert_eq!(AccountHistorySpec::PRUNE_SEGMENT, PruneSegment::AccountHistory);
        assert_eq!(
            AccountHistorySpec::CHANGESET_STATIC_SEGMENT,
            StaticFileSegment::AccountChangeSets
        );

        assert_eq!(StorageHistorySpec::STAGE_ID, StageId::IndexStorageHistory);
        assert_eq!(StorageHistorySpec::PRUNE_SEGMENT, PruneSegment::StorageHistory);
        assert_eq!(
            StorageHistorySpec::CHANGESET_STATIC_SEGMENT,
            StaticFileSegment::StorageChangeSets
        );
    }

    #[test]
    fn history_spec_settings_check() {
        let legacy = StorageSettings::legacy();
        assert!(!AccountHistorySpec::history_in_rocksdb(&legacy));
        assert!(!StorageHistorySpec::history_in_rocksdb(&legacy));
        assert!(!AccountHistorySpec::changesets_in_static_files(&legacy));
        assert!(!StorageHistorySpec::changesets_in_static_files(&legacy));

        let with_account = legacy.with_account_history_in_rocksdb(true);
        assert!(AccountHistorySpec::history_in_rocksdb(&with_account));
        assert!(!StorageHistorySpec::history_in_rocksdb(&with_account));

        let with_storage = legacy.with_storages_history_in_rocksdb(true);
        assert!(!AccountHistorySpec::history_in_rocksdb(&with_storage));
        assert!(StorageHistorySpec::history_in_rocksdb(&with_storage));

        let with_account_cs = legacy.with_account_changesets_in_static_files(true);
        assert!(AccountHistorySpec::changesets_in_static_files(&with_account_cs));
        assert!(!StorageHistorySpec::changesets_in_static_files(&with_account_cs));

        let with_storage_cs = legacy.with_storage_changesets_in_static_files(true);
        assert!(!AccountHistorySpec::changesets_in_static_files(&with_storage_cs));
        assert!(StorageHistorySpec::changesets_in_static_files(&with_storage_cs));
    }
}
