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
use reth_storage_api::{BalNotification, BalNotificationStream, BalStore, RawBal};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_tokio_util::EventSender;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

/// RocksDB-backed BAL store.
///
/// Persisted BALs are keyed by `(block_number, block_hash)`. Hash-only lookups only read the
/// pending buffer.
#[derive(Clone)]
pub struct RocksDBBalStore {
    retention: PruneMode,
    rocksdb: RocksDBProvider,
    buffer: Arc<RwLock<RocksDBBalStoreBuffer>>,
    notifications: EventSender<BalNotification>,
}

impl RocksDBBalStore {
    /// Creates a new store with the default retention distance.
    pub fn new(rocksdb: RocksDBProvider) -> Self {
        Self::with_retention_distance(rocksdb, BAL_RETENTION_PERIOD_SLOTS)
    }

    /// Creates a new store with the given retention distance.
    pub fn with_retention_distance(rocksdb: RocksDBProvider, blocks: u64) -> Self {
        Self {
            retention: PruneMode::Distance(blocks),
            rocksdb,
            buffer: Arc::new(RwLock::new(RocksDBBalStoreBuffer::default())),
            notifications: EventSender::new(super::DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE),
        }
    }

    #[cfg(test)]
    fn rocksdb_provider(&self) -> &RocksDBProvider {
        &self.rocksdb
    }

    fn keys_to_prune(&self, tip: BlockNumber) -> ProviderResult<Vec<StoredBlockAccessListKey>> {
        let mut keys = Vec::new();
        let iter = self.rocksdb.raw_key_iter_from::<tables::BlockAccessLists>(
            StoredBlockAccessListKey::first_at_number(0),
        )?;

        for key_bytes in iter {
            let key_bytes = key_bytes?;
            let key = StoredBlockAccessListKey::decode(&key_bytes)
                .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
            if !self.retention.should_prune(key.number(), tip) {
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
            .field("retention", &self.retention)
            .field("rocksdb", &self.rocksdb)
            .finish_non_exhaustive()
    }
}

/// Buffered BALs waiting to be flushed to RocksDB.
#[derive(Debug, Default)]
struct RocksDBBalStoreBuffer {
    // Hash index for serving hash-only lookups before flush.
    entries: HashMap<BlockHash, RocksDBBalEntry>,
    // Block-number index for pruning buffered entries.
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    // Ordered writes waiting for flush.
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

    fn keys_to_prune(
        &self,
        prune_mode: PruneMode,
        tip: BlockNumber,
    ) -> Vec<StoredBlockAccessListKey> {
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

/// Buffered BAL entry with its block number.
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
            let keys = buffer.keys_to_prune(self.retention, highest_block_number);
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
            let keys = buffer.keys_to_prune(self.retention, highest_block_number);
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
            let value = StoredBlockAccessList::new(bal.clone());
            batch.put::<tables::BlockAccessLists>(*key, &value)?;
        }
        batch.commit()?;

        buffer.remove_flushed(&pending);
        Ok(())
    }

    fn prune(&self, tip: BlockNumber) -> ProviderResult<usize> {
        let disk_keys = self.keys_to_prune(tip)?;
        let mut buffer = self.buffer.write();
        let buffer_keys = buffer.keys_to_prune(self.retention, tip);
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

    fn get_by_block_num_hash(&self, block: NumHash) -> ProviderResult<Option<Bytes>> {
        self.read_one(block)
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

    fn read_many(store: &RocksDBBalStore, blocks: &[NumHash]) -> Vec<Option<Bytes>> {
        blocks.iter().map(|block| store.get_by_block_num_hash(*block).unwrap()).collect()
    }

    #[test]
    fn inserts_and_reads_by_block_num_hash() {
        let (_dir, store) = test_store();
        let hash = B256::random();
        let missing = NumHash::new(1, B256::random());
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(NumHash::new(1, hash), RawBal::from(bal.clone())).unwrap();

        assert_eq!(read_many(&store, &[NumHash::new(1, hash), missing]), vec![Some(bal), None]);
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
        assert_eq!(read_many(&store, &[block_1, block_2]), vec![Some(bal_1), Some(bal_2.clone())]);
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
            read_many(&store, &[NumHash::new(10, hash_a), NumHash::new(10, hash_b)]),
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
            read_many(&store, &[NumHash::new(2, hash_a), NumHash::new(200, hash_b)]),
            vec![Some(bal_a), Some(bal_b)]
        );
    }

    #[test]
    fn missing_and_empty_bal_are_distinct() {
        let (_dir, store) = test_store();
        let empty_hash = B256::with_last_byte(1);
        let missing_hash = B256::with_last_byte(2);
        let empty_bal = Bytes::from_static(&[0xc0]);

        store.insert(NumHash::new(1, empty_hash), RawBal::from(empty_bal.clone())).unwrap();

        assert_eq!(
            read_many(&store, &[NumHash::new(1, empty_hash), NumHash::new(1, missing_hash)]),
            vec![Some(empty_bal), None]
        );
    }

    #[test]
    fn prune_uses_configured_retention() {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb = RocksDBBuilder::new(dir.path()).with_default_tables().build().unwrap();
        let store = RocksDBBalStore::with_retention_distance(rocksdb, 2);
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
            read_many(&store, &[NumHash::new(7, old_hash), NumHash::new(8, retained_hash)]),
            vec![None, Some(retained_bal)]
        );
    }

    #[test]
    fn corrupt_payload_hash_is_not_served() {
        let (_dir, store) = test_store();
        let block = NumHash::new(1, B256::with_last_byte(1));
        let key = StoredBlockAccessListKey::new(block);
        let mut encoded = Vec::new();
        encoded.extend_from_slice(B256::ZERO.as_slice());
        encoded.extend_from_slice(&[0xc0]);
        let value = StoredBlockAccessList::decompress(&encoded).unwrap();

        store.rocksdb_provider().put::<tables::BlockAccessLists>(key, &value).unwrap();

        assert!(store.get_by_block_num_hash(block).is_err());
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
