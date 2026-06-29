use crate::providers::RocksDBProvider;
use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use alloy_eips::NumHash;
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_db_api::{
    models::{StoredBlockAccessList, StoredBlockAccessListKey},
    table::{Decode, Decompress},
    tables, DatabaseError,
};
use reth_prune_types::PruneMode;
use reth_storage_api::{
    BalNotification, BalNotificationStream, BalStore, GetBlockAccessListLimit, RawBal,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_tokio_util::EventSender;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

/// RocksDB-backed BAL store keyed by `(block_number, block_hash)`.
///
/// Hash-only lookups are served from the pending in-memory overlay. Persisted disk lookups require
/// the block number/hash pair.
#[derive(Clone)]
pub struct RocksDBBalStore {
    config: RocksDBBalStoreConfig,
    rocksdb: RocksDBProvider,
    buffer: Arc<RwLock<RocksDBBalStoreBuffer>>,
    notifications: EventSender<BalNotification>,
}

impl RocksDBBalStore {
    /// Creates a new RocksDB-backed BAL store with the default config.
    ///
    /// The provider must be built with the `BlockAccessLists` column family.
    pub fn new(rocksdb: RocksDBProvider) -> Self {
        Self::with_config(rocksdb, RocksDBBalStoreConfig::default())
    }

    /// Creates a new RocksDB-backed BAL store with the given config.
    ///
    /// The provider must be built with the `BlockAccessLists` column family.
    pub fn with_config(rocksdb: RocksDBProvider, config: RocksDBBalStoreConfig) -> Self {
        Self {
            config,
            rocksdb,
            buffer: Arc::new(RwLock::new(RocksDBBalStoreBuffer::default())),
            notifications: EventSender::new(super::DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE),
        }
    }

    #[cfg(test)]
    fn rocksdb_provider(&self) -> &RocksDBProvider {
        &self.rocksdb
    }

    fn keys_through_block(
        &self,
        to_block: BlockNumber,
    ) -> ProviderResult<Vec<StoredBlockAccessListKey>> {
        let mut keys = Vec::new();
        let end = StoredBlockAccessListKey::after_number(to_block);
        let iter = self.rocksdb.raw_key_iter_from::<tables::BlockAccessLists>(
            StoredBlockAccessListKey::first_at_number(0),
        )?;

        for key_bytes in iter {
            let key_bytes = key_bytes?;
            let key = StoredBlockAccessListKey::decode(&key_bytes)
                .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
            if end.is_some_and(|end| key >= end) {
                break
            }
            keys.push(key);
        }

        Ok(keys)
    }

    fn keys_to_prune(&self, tip: BlockNumber) -> ProviderResult<Vec<StoredBlockAccessListKey>> {
        let Some(prune_mode) = self.config.retention else { return Ok(Vec::new()) };

        let mut keys = Vec::new();
        let iter = self.rocksdb.raw_key_iter_from::<tables::BlockAccessLists>(
            StoredBlockAccessListKey::first_at_number(0),
        )?;

        for key_bytes in iter {
            let key_bytes = key_bytes?;
            let key = StoredBlockAccessListKey::decode(&key_bytes)
                .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
            if !prune_mode.should_prune(key.number(), tip) {
                break
            }
            keys.push(key);
        }

        Ok(keys)
    }

    fn delete_keys(&self, keys: Vec<StoredBlockAccessListKey>) -> ProviderResult<usize> {
        if keys.is_empty() {
            return Ok(0)
        }

        let mut batch = self.rocksdb.batch();
        for key in &keys {
            batch.delete::<tables::BlockAccessLists>(*key)?;
        }
        batch.commit()?;
        Ok(keys.len())
    }

    fn read_one_from_disk(&self, key: StoredBlockAccessListKey) -> ProviderResult<Option<Bytes>> {
        let Some(value) = self.rocksdb.get_raw::<tables::BlockAccessLists>(key)? else {
            return Ok(None)
        };
        let stored = StoredBlockAccessList::decompress(&value)
            .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
        stored.into_verified_raw().map(|raw| Some(raw.into_raw())).map_err(ProviderError::other)
    }

    fn read_one(&self, block: NumHash) -> ProviderResult<Option<Bytes>> {
        if let Some(bal) = self.buffer.read().get_by_block_num_hash(block) {
            return Ok(Some(bal))
        }

        self.read_one_from_disk(StoredBlockAccessListKey::new(block))
    }
}

impl std::fmt::Debug for RocksDBBalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBBalStore")
            .field("config", &self.config)
            .field("rocksdb", &self.rocksdb)
            .finish_non_exhaustive()
    }
}

/// Configuration for [`RocksDBBalStore`].
///
/// Retention controls logical BAL availability. RocksDB reclaims deleted BlobDB payload space later
/// through compaction and blob garbage collection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RocksDBBalStoreConfig {
    /// Retention policy for on-disk BALs.
    retention: Option<PruneMode>,
}

impl RocksDBBalStoreConfig {
    /// Returns a config that keeps on-disk BALs within the given block distance.
    pub const fn with_retention_distance(blocks: u64) -> Self {
        Self { retention: Some(PruneMode::Distance(blocks)) }
    }

    /// Returns a config with no on-disk BAL retention limit.
    pub const fn unbounded() -> Self {
        Self { retention: None }
    }
}

impl Default for RocksDBBalStoreConfig {
    fn default() -> Self {
        Self::with_retention_distance(BAL_RETENTION_PERIOD_SLOTS)
    }
}

#[derive(Debug, Default)]
struct RocksDBBalStoreBuffer {
    entries: HashMap<BlockHash, RocksDBBalEntry>,
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    pending: BTreeMap<StoredBlockAccessListKey, RawBal>,
    highest_block_number: Option<BlockNumber>,
}

impl RocksDBBalStoreBuffer {
    fn insert(&mut self, block: NumHash, bal: RawBal) {
        let pending = bal.clone();
        if let Some(entry) =
            self.entries.insert(block.hash, RocksDBBalEntry { block_number: block.number, bal })
        {
            self.remove_hash_from_number(entry.block_number, block.hash);
            self.pending.remove(&StoredBlockAccessListKey::new(NumHash::new(
                entry.block_number,
                block.hash,
            )));
        }

        self.hashes_by_number.entry(block.number).or_default().push(block.hash);
        self.pending.insert(StoredBlockAccessListKey::new(block), pending);
        self.highest_block_number = Some(
            self.highest_block_number.map_or(block.number, |highest| highest.max(block.number)),
        );
    }

    fn pending_entries_through(
        &self,
        to_block: BlockNumber,
    ) -> Vec<(StoredBlockAccessListKey, RawBal)> {
        self.pending
            .iter()
            .take_while(|(key, _)| key.number() <= to_block)
            .map(|(key, bal)| (*key, bal.clone()))
            .collect()
    }

    fn keys_through_block(&self, to_block: BlockNumber) -> Vec<StoredBlockAccessListKey> {
        self.hashes_by_number
            .iter()
            .take_while(|(block_number, _)| **block_number <= to_block)
            .flat_map(|(block_number, hashes)| {
                hashes.iter().map(move |hash| {
                    StoredBlockAccessListKey::new(NumHash::new(*block_number, *hash))
                })
            })
            .collect()
    }

    fn keys_to_prune(
        &self,
        prune_mode: Option<PruneMode>,
        tip: BlockNumber,
    ) -> Vec<StoredBlockAccessListKey> {
        let Some(prune_mode) = prune_mode else { return Vec::new() };

        self.hashes_by_number
            .iter()
            .take_while(|(block_number, _)| prune_mode.should_prune(**block_number, tip))
            .flat_map(|(block_number, hashes)| {
                hashes.iter().map(move |hash| {
                    StoredBlockAccessListKey::new(NumHash::new(*block_number, *hash))
                })
            })
            .collect()
    }

    fn get_by_hash(&self, hash: BlockHash) -> Option<Bytes> {
        self.entries.get(&hash).map(|entry| entry.bal.as_raw().clone())
    }

    fn get_by_block_num_hash(&self, block: NumHash) -> Option<Bytes> {
        self.entries
            .get(&block.hash)
            .filter(|entry| entry.block_number == block.number)
            .map(|entry| entry.bal.as_raw().clone())
    }

    fn remove_flushed(&mut self, flushed: &[(StoredBlockAccessListKey, RawBal)]) {
        for (key, bal) in flushed {
            let pending_matches =
                self.pending.get(key).is_some_and(|pending| pending.as_raw() == bal.as_raw());
            if pending_matches {
                self.pending.remove(key);
            }

            let block = key.num_hash();
            let entry_matches = self.entries.get(&block.hash).is_some_and(|entry| {
                entry.block_number == block.number && entry.bal.as_raw() == bal.as_raw()
            });
            if entry_matches {
                self.entries.remove(&block.hash);
                self.remove_hash_from_number(block.number, block.hash);
            }
        }
    }

    fn remove_keys(&mut self, keys: &[StoredBlockAccessListKey]) -> usize {
        let mut removed = 0;
        for key in keys {
            let block = key.num_hash();
            let pending_removed = self.pending.remove(key).is_some();
            let entry_removed = if self
                .entries
                .get(&block.hash)
                .is_some_and(|entry| entry.block_number == block.number)
            {
                self.entries.remove(&block.hash).is_some()
            } else {
                false
            };

            if entry_removed {
                self.remove_hash_from_number(block.number, block.hash);
            }
            removed += usize::from(pending_removed || entry_removed);
        }
        removed
    }

    fn remove_hash_from_number(&mut self, block_number: BlockNumber, block_hash: BlockHash) {
        let empty = self.hashes_by_number.get_mut(&block_number).is_some_and(|hashes| {
            hashes.retain(|hash| *hash != block_hash);
            hashes.is_empty()
        });
        if empty {
            self.hashes_by_number.remove(&block_number);
        }
    }
}

#[derive(Debug)]
struct RocksDBBalEntry {
    block_number: BlockNumber,
    bal: RawBal,
}

impl BalStore for RocksDBBalStore {
    fn insert(&self, block: NumHash, bal: RawBal) -> ProviderResult<()> {
        let mut buffer = self.buffer.write();
        buffer.insert(block, bal.clone());
        if let Some(highest_block_number) = buffer.highest_block_number {
            let keys = buffer.keys_to_prune(self.config.retention, highest_block_number);
            buffer.remove_keys(&keys);
        }
        drop(buffer);

        self.notifications.notify(BalNotification::new(block, bal));
        Ok(())
    }

    fn insert_many(&self, entries: Vec<(NumHash, RawBal)>) -> ProviderResult<()> {
        if entries.is_empty() {
            return Ok(())
        }

        let mut buffer = self.buffer.write();
        buffer.entries.reserve(entries.len());
        for (block, bal) in &entries {
            buffer.insert(*block, bal.clone());
        }
        if let Some(highest_block_number) = buffer.highest_block_number {
            let keys = buffer.keys_to_prune(self.config.retention, highest_block_number);
            buffer.remove_keys(&keys);
        }
        drop(buffer);

        for (block, bal) in entries {
            self.notifications.notify(BalNotification::new(block, bal));
        }
        Ok(())
    }

    fn flush(&self, to_block: BlockNumber) -> ProviderResult<()> {
        let mut buffer = self.buffer.write();
        let pending = buffer.pending_entries_through(to_block);
        if pending.is_empty() {
            return Ok(())
        }

        let mut batch = self.rocksdb.batch();
        for (key, bal) in &pending {
            let value = StoredBlockAccessList::new_unchecked(bal.clone(), bal.hash());
            batch.put::<tables::BlockAccessLists>(*key, &value)?;
        }
        batch.commit()?;

        buffer.remove_flushed(&pending);
        Ok(())
    }

    fn prune(&self, tip: BlockNumber) -> ProviderResult<usize> {
        let disk_keys = self.keys_to_prune(tip)?;
        let mut buffer = self.buffer.write();
        let buffer_keys = buffer.keys_to_prune(self.config.retention, tip);
        let pruned =
            disk_keys.iter().chain(buffer_keys.iter()).copied().collect::<BTreeSet<_>>().len();

        self.delete_keys(disk_keys)?;
        buffer.remove_keys(&buffer_keys);
        Ok(pruned)
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let buffer = self.buffer.read();
        Ok(block_hashes.iter().map(|hash| buffer.get_by_hash(*hash)).collect())
    }

    fn get_by_block_num_hashes(&self, blocks: &[NumHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let mut out = Vec::with_capacity(blocks.len());
        for block in blocks {
            out.push(self.read_one(*block)?);
        }
        Ok(out)
    }

    fn append_by_block_num_hashes_with_limit(
        &self,
        blocks: &[NumHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Option<Bytes>>,
    ) -> ProviderResult<()> {
        let mut size = 0;
        for block in blocks {
            let bal = self.read_one(*block)?;
            size += bal.as_ref().map_or(1, |bytes| bytes.len());
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }
        Ok(())
    }

    fn delete_range_by_number(&self, to_block: BlockNumber) -> ProviderResult<usize> {
        let disk_keys = self.keys_through_block(to_block)?;
        let mut buffer = self.buffer.write();
        let buffer_keys = buffer.keys_through_block(to_block);
        let deleted =
            disk_keys.iter().chain(buffer_keys.iter()).copied().collect::<BTreeSet<_>>().len();

        self.delete_keys(disk_keys)?;
        buffer.remove_keys(&buffer_keys);
        Ok(deleted)
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.notifications.new_listener()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::RocksDBBuilder;
    use alloy_primitives::B256;
    use reth_db_api::table::Encode;
    use tokio_stream::StreamExt;

    fn test_store() -> (tempfile::TempDir, RocksDBBalStore) {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb = RocksDBBuilder::new(dir.path()).with_default_tables().build().unwrap();
        (dir, RocksDBBalStore::new(rocksdb))
    }

    fn disk_bal(store: &RocksDBBalStore, block: NumHash) -> Option<Bytes> {
        store
            .rocksdb_provider()
            .get_raw::<tables::BlockAccessLists>(StoredBlockAccessListKey::new(block))
            .unwrap()
            .map(|value| {
                StoredBlockAccessList::decompress(&value)
                    .unwrap()
                    .into_verified_raw()
                    .unwrap()
                    .into_raw()
            })
    }

    #[test]
    fn inserts_and_reads_by_block_num_hash() {
        let (_dir, store) = test_store();
        let hash = B256::random();
        let missing = NumHash::new(1, B256::random());
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(NumHash::new(1, hash), RawBal::from(bal.clone())).unwrap();

        assert_eq!(
            store.get_by_block_num_hashes(&[NumHash::new(1, hash), missing]).unwrap(),
            vec![Some(bal), None]
        );
    }

    #[test]
    fn hash_only_lookup_reads_pending_buffer() {
        let (_dir, store) = test_store();
        let block = NumHash::new(1, B256::random());
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(block, RawBal::from(bal.clone())).unwrap();

        assert_eq!(store.get_by_hashes(&[block.hash]).unwrap(), vec![Some(bal)]);
    }

    #[test]
    fn flush_writes_pending_bals_through_block() {
        let (_dir, store) = test_store();
        let block_1 = NumHash::new(1, B256::with_last_byte(1));
        let block_2 = NumHash::new(2, B256::with_last_byte(2));
        let bal_1 = Bytes::from_static(&[0xc1, 0x01]);
        let bal_2 = Bytes::from_static(&[0xc1, 0x02]);

        store.insert(block_1, RawBal::from(bal_1.clone())).unwrap();
        store.insert(block_2, RawBal::from(bal_2.clone())).unwrap();

        assert_eq!(disk_bal(&store, block_1), None);
        assert_eq!(disk_bal(&store, block_2), None);

        store.flush(1).unwrap();

        assert_eq!(disk_bal(&store, block_1), Some(bal_1.clone()));
        assert_eq!(disk_bal(&store, block_2), None);
        assert_eq!(
            store.get_by_block_num_hashes(&[block_1, block_2]).unwrap(),
            vec![Some(bal_1), Some(bal_2.clone())]
        );
        assert_eq!(
            store.get_by_hashes(&[block_1.hash, block_2.hash]).unwrap(),
            vec![None, Some(bal_2)]
        );
    }

    #[test]
    fn keeps_multiple_hashes_at_same_number() {
        let (_dir, store) = test_store();
        let hash_a = B256::with_last_byte(1);
        let hash_b = B256::with_last_byte(2);
        let bal_a = Bytes::from_static(&[0xc1, 0x01]);
        let bal_b = Bytes::from_static(&[0xc1, 0x02]);

        store.insert(NumHash::new(10, hash_a), RawBal::from(bal_a.clone())).unwrap();
        store.insert(NumHash::new(10, hash_b), RawBal::from(bal_b.clone())).unwrap();

        assert_eq!(
            store
                .get_by_block_num_hashes(&[NumHash::new(10, hash_a), NumHash::new(10, hash_b)])
                .unwrap(),
            vec![Some(bal_a), Some(bal_b)]
        );
    }

    #[test]
    fn sparse_numbers_are_valid() {
        let (_dir, store) = test_store();
        let hash_a = B256::with_last_byte(1);
        let hash_b = B256::with_last_byte(2);
        let bal_a = Bytes::from_static(&[0xc1, 0x01]);
        let bal_b = Bytes::from_static(&[0xc1, 0x02]);

        store.insert(NumHash::new(2, hash_a), RawBal::from(bal_a.clone())).unwrap();
        store.insert(NumHash::new(200, hash_b), RawBal::from(bal_b.clone())).unwrap();

        assert_eq!(
            store
                .get_by_block_num_hashes(&[NumHash::new(2, hash_a), NumHash::new(200, hash_b)])
                .unwrap(),
            vec![Some(bal_a), Some(bal_b)]
        );
    }

    #[test]
    fn delete_range_by_number_removes_all_hashes_at_old_heights() {
        let (_dir, store) = test_store();
        let old_a = B256::with_last_byte(1);
        let old_b = B256::with_last_byte(2);
        let new_hash = B256::with_last_byte(3);
        let new_bal = Bytes::from_static(&[0xc1, 0x03]);

        store
            .insert(NumHash::new(7, old_a), RawBal::from(Bytes::from_static(&[0xc1, 0x01])))
            .unwrap();
        store
            .insert(NumHash::new(7, old_b), RawBal::from(Bytes::from_static(&[0xc1, 0x02])))
            .unwrap();
        store.insert(NumHash::new(9, new_hash), RawBal::from(new_bal.clone())).unwrap();
        store.flush(9).unwrap();

        assert_eq!(store.delete_range_by_number(7).unwrap(), 2);
        assert_eq!(disk_bal(&store, NumHash::new(7, old_a)), None);
        assert_eq!(disk_bal(&store, NumHash::new(7, old_b)), None);
        assert_eq!(disk_bal(&store, NumHash::new(9, new_hash)), Some(new_bal.clone()));
        assert_eq!(
            store
                .get_by_block_num_hashes(&[
                    NumHash::new(7, old_a),
                    NumHash::new(7, old_b),
                    NumHash::new(9, new_hash),
                ])
                .unwrap(),
            vec![None, None, Some(new_bal)]
        );
    }

    #[test]
    fn delete_range_by_number_removes_read_payloads() {
        let (_dir, store) = test_store();
        let block = NumHash::new(7, B256::with_last_byte(1));
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(block, RawBal::from(bal.clone())).unwrap();
        assert_eq!(store.get_by_block_num_hash(block).unwrap(), Some(bal));

        assert_eq!(store.delete_range_by_number(7).unwrap(), 1);
        assert_eq!(store.get_by_block_num_hash(block).unwrap(), None);
    }

    #[test]
    fn missing_and_empty_bal_are_distinct() {
        let (_dir, store) = test_store();
        let empty_hash = B256::with_last_byte(1);
        let missing_hash = B256::with_last_byte(2);
        let empty_bal = Bytes::from_static(&[0xc0]);

        store.insert(NumHash::new(1, empty_hash), RawBal::from(empty_bal.clone())).unwrap();

        assert_eq!(
            store
                .get_by_block_num_hashes(&[
                    NumHash::new(1, empty_hash),
                    NumHash::new(1, missing_hash),
                ])
                .unwrap(),
            vec![Some(empty_bal), None]
        );
    }

    #[test]
    fn prune_uses_configured_retention() {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb = RocksDBBuilder::new(dir.path()).with_default_tables().build().unwrap();
        let store = RocksDBBalStore::with_config(
            rocksdb,
            RocksDBBalStoreConfig::with_retention_distance(2),
        );
        let old_hash = B256::with_last_byte(1);
        let retained_hash = B256::with_last_byte(2);
        let retained_bal = Bytes::from_static(&[0xc1, 0x02]);

        store
            .insert(NumHash::new(7, old_hash), RawBal::from(Bytes::from_static(&[0xc1, 0x01])))
            .unwrap();
        store.insert(NumHash::new(8, retained_hash), RawBal::from(retained_bal.clone())).unwrap();
        store.flush(8).unwrap();

        assert_eq!(store.prune(10).unwrap(), 1);
        assert_eq!(disk_bal(&store, NumHash::new(7, old_hash)), None);
        assert_eq!(disk_bal(&store, NumHash::new(8, retained_hash)), Some(retained_bal.clone()));
        assert_eq!(
            store
                .get_by_block_num_hashes(&[
                    NumHash::new(7, old_hash),
                    NumHash::new(8, retained_hash)
                ])
                .unwrap(),
            vec![None, Some(retained_bal)]
        );
    }

    #[test]
    fn corrupt_payload_hash_is_not_served() {
        let (_dir, store) = test_store();
        let block = NumHash::new(1, B256::with_last_byte(1));
        let key = StoredBlockAccessListKey::new(block);
        let value = StoredBlockAccessList::new_unchecked(
            RawBal::from(Bytes::from_static(&[0xc0])),
            B256::ZERO,
        );

        store.rocksdb_provider().put::<tables::BlockAccessLists>(key, &value).unwrap();

        assert!(store.get_by_block_num_hash(block).is_err());
    }

    #[test]
    fn key_ordering_matches_number_then_hash() {
        let key_1_high_hash =
            StoredBlockAccessListKey::new(NumHash::new(1, B256::repeat_byte(0xff))).encode();
        let key_2_low_hash = StoredBlockAccessListKey::new(NumHash::new(2, B256::ZERO)).encode();

        assert!(key_1_high_hash < key_2_low_hash);
    }

    #[tokio::test]
    async fn insert_notifies_subscribers() {
        let (_dir, store) = test_store();
        let mut stream = store.bal_stream();
        let block = NumHash::new(1, B256::with_last_byte(1));
        let bal = RawBal::from(Bytes::from_static(&[0xc0]));

        store.insert(block, bal.clone()).unwrap();

        assert_eq!(stream.next().await.unwrap(), BalNotification::new(block, bal));
    }
}
