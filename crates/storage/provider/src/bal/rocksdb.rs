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

/// Number of recent blocks kept in the in-memory `RocksDB` BAL buffer.
const DEFAULT_BAL_BUFFER_RETENTION_DISTANCE: u64 = 32;

/// RocksDB-backed BAL store.
///
/// Persisted BALs are keyed by `(block_number, block_hash)` for ordered pruning and indexed by
/// block hash for direct [`BalStore`] lookups. Validated BALs enter a shared in-memory buffer
/// first; [`BalStore::flush`] makes confirmed canonical entries durable while retaining them in the
/// read cache until its shorter retention window expires.
#[derive(Clone)]
pub struct RocksDBBalStore {
    /// Number of recent blocks retained in the in-memory buffer.
    buffer_retention_distance: u64,
    /// `RocksDB` provider used for persisted BAL reads and writes.
    rocksdb: RocksDBProvider,
    /// Shared recent-read cache and pending-write state.
    buffer: Arc<RwLock<RocksDBBalStoreBuffer>>,
    /// Broadcasts BAL insert notifications.
    notifications: EventSender<BalNotification>,
}

impl RocksDBBalStore {
    /// Creates a new store with the EIP-defined retention distance.
    pub fn new(rocksdb: RocksDBProvider) -> Self {
        Self::with_buffer_retention_distance(rocksdb, DEFAULT_BAL_BUFFER_RETENTION_DISTANCE)
    }

    /// Creates a new store that retains buffered BALs for the given block distance.
    ///
    /// This does not change the EIP-defined retention distance for persisted BALs.
    pub fn with_buffer_retention_distance(rocksdb: RocksDBProvider, blocks: u64) -> Self {
        Self {
            buffer_retention_distance: blocks,
            rocksdb,
            buffer: Arc::new(RwLock::new(RocksDBBalStoreBuffer::default())),
            notifications: EventSender::new(super::DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE),
        }
    }

    #[cfg(test)]
    const fn rocksdb_provider(&self) -> &RocksDBProvider {
        &self.rocksdb
    }

    fn keys_to_prune(&self, tip: BlockNumber) -> ProviderResult<Vec<StoredBlockAccessListKey>> {
        let retention = PruneMode::Distance(BAL_RETENTION_PERIOD_SLOTS);
        let mut keys = Vec::new();
        let iter = self.rocksdb.raw_key_iter_from::<tables::BlockAccessLists>(
            StoredBlockAccessListKey::first_at_number(0),
        )?;

        for key_bytes in iter {
            let key_bytes = key_bytes?;
            let key = StoredBlockAccessListKey::decode(&key_bytes)
                .map_err(|_| ProviderError::Database(DatabaseError::Decode))?;
            if !retention.should_prune(key.number(), tip) {
                break
            }
            keys.push(key);
        }

        Ok(keys)
    }

    fn delete_keys(&self, keys: &[StoredBlockAccessListKey]) -> ProviderResult<usize> {
        if keys.is_empty() {
            return Ok(0)
        }

        let mut batch = self.rocksdb.batch();
        for key in keys {
            batch.delete::<tables::BlockAccessLists>(*key)?;
            batch.delete::<tables::BlockAccessListBlockNumbers>(key.hash())?;
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
        Ok(Some(stored.into_raw()))
    }

    fn read_one_by_hash(&self, block_hash: BlockHash) -> ProviderResult<Option<Bytes>> {
        if let Some(bal) = self.buffer.read().get_by_hash(block_hash) {
            return Ok(Some(bal))
        }

        let Some(block_number) =
            self.rocksdb.get::<tables::BlockAccessListBlockNumbers>(block_hash)?
        else {
            return Ok(None)
        };
        self.read_one_from_disk(StoredBlockAccessListKey::new(block_number, block_hash))
    }
}

impl std::fmt::Debug for RocksDBBalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RocksDBBalStore")
            .field("buffer_retention_distance", &self.buffer_retention_distance)
            .field("rocksdb", &self.rocksdb)
            .finish_non_exhaustive()
    }
}

impl BalStore for RocksDBBalStore {
    fn insert(&self, block: NumHash, bal: RawBal) -> ProviderResult<()> {
        let mut buffer = self.buffer.write();
        buffer.insert(block, bal.clone());
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
        drop(buffer);

        for (block, bal) in entries {
            self.notifications.notify(BalNotification::new(block, bal));
        }
        Ok(())
    }

    fn flush(&self, blocks: &[NumHash]) -> ProviderResult<()> {
        let pending = {
            let mut buffer = self.buffer.write();
            buffer.mark_canonical(blocks);
            buffer.canonical_pending_entries()
        };
        if !pending.is_empty() {
            let mut batch = self.rocksdb.batch();
            for (key, bal) in &pending {
                let value = StoredBlockAccessList::new_unchecked(bal.hash(), bal.as_raw().clone());
                batch.put::<tables::BlockAccessLists>(*key, &value)?;
                batch.put::<tables::BlockAccessListBlockNumbers>(key.hash(), &key.number())?;
            }
            batch.commit()?;

            self.buffer.write().remove_flushed_pending(&pending);
        }

        if let Some(tip) = blocks.iter().map(|block| block.number).max() {
            self.buffer.write().prune(self.buffer_retention_distance, tip);
        }
        Ok(())
    }

    fn prune(&self, tip: BlockNumber) -> ProviderResult<usize> {
        let keys = self.keys_to_prune(tip)?;
        let pruned = self.delete_keys(&keys)?;
        self.buffer.write().remove_keys(&keys);
        Ok(pruned)
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        block_hashes.iter().map(|hash| self.read_one_by_hash(*hash)).collect()
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Option<Bytes>>,
    ) -> ProviderResult<()> {
        let mut size = 0;
        for block_hash in block_hashes {
            let bal = self.read_one_by_hash(*block_hash)?;
            size += bal.as_ref().map_or(1, |bytes| bytes.len());
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }
        Ok(())
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.notifications.new_listener()
    }
}

/// Shared in-memory state for recent reads and writes awaiting canonical confirmation.
///
/// Successful flushes clear only the pending-write state. Cached entries remain available until
/// the buffer retention window evicts them.
#[derive(Debug, Default)]
struct RocksDBBalStoreBuffer {
    /// Hash index for serving recent hash-only lookups.
    entries: HashMap<BlockHash, RocksDBBalEntry>,
    /// Block-number index for pruning buffered entries.
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    /// Validated BALs waiting to be confirmed canonical and flushed.
    pending: BTreeMap<StoredBlockAccessListKey, RawBal>,
    /// Pending writes confirmed canonical, including writes retained for retry after failure.
    canonical_pending: BTreeSet<StoredBlockAccessListKey>,
}

impl RocksDBBalStoreBuffer {
    fn insert(&mut self, block: NumHash, bal: RawBal) {
        let pending = bal.clone();
        if let Some(entry) =
            self.entries.insert(block.hash, RocksDBBalEntry { block_number: block.number, bal })
        {
            self.remove_hash_from_number(entry.block_number, block.hash);
            self.pending.remove(&StoredBlockAccessListKey::new(entry.block_number, block.hash));
            self.canonical_pending
                .remove(&StoredBlockAccessListKey::new(entry.block_number, block.hash));
        }

        self.hashes_by_number.entry(block.number).or_default().push(block.hash);
        self.pending.insert(StoredBlockAccessListKey::new(block.number, block.hash), pending);
    }

    /// Marks exact pending block identities as eligible for the next flush.
    fn mark_canonical(&mut self, blocks: &[NumHash]) {
        self.canonical_pending.extend(
            blocks
                .iter()
                .map(|block| StoredBlockAccessListKey::new(block.number, block.hash))
                .filter(|key| self.pending.contains_key(key)),
        );
    }

    /// Snapshots confirmed writes so `RocksDB` I/O can run without holding the buffer lock.
    fn canonical_pending_entries(&self) -> Vec<(StoredBlockAccessListKey, RawBal)> {
        self.canonical_pending
            .iter()
            .filter_map(|key| self.pending.get(key).map(|bal| (*key, bal.clone())))
            .collect()
    }

    fn keys_to_prune(
        &self,
        retention_distance: u64,
        tip: BlockNumber,
    ) -> Vec<StoredBlockAccessListKey> {
        let prune_mode = PruneMode::Distance(retention_distance);
        self.hashes_by_number
            .iter()
            .take_while(|(block_number, _)| prune_mode.should_prune(**block_number, tip))
            .flat_map(|(block_number, hashes)| {
                hashes.iter().map(move |hash| StoredBlockAccessListKey::new(*block_number, *hash))
            })
            .filter(|key| !self.canonical_pending.contains(key))
            .collect()
    }

    fn get_by_hash(&self, hash: BlockHash) -> Option<Bytes> {
        self.entries.get(&hash).map(|entry| entry.bal.as_raw().clone())
    }

    /// Clears pending state only if it still matches the snapshot written to `RocksDB`.
    ///
    /// A concurrent replacement for the same key must remain pending for a later flush.
    fn remove_flushed_pending(&mut self, flushed: &[(StoredBlockAccessListKey, RawBal)]) {
        for (key, bal) in flushed {
            let pending_matches =
                self.pending.get(key).is_some_and(|pending| pending.as_raw() == bal.as_raw());
            if pending_matches {
                self.pending.remove(key);
                self.canonical_pending.remove(key);
            }
        }
    }

    fn prune(&mut self, retention_distance: u64, tip: BlockNumber) {
        let keys = self.keys_to_prune(retention_distance, tip);
        self.remove_keys(&keys);
    }

    fn remove_keys(&mut self, keys: &[StoredBlockAccessListKey]) -> usize {
        let mut removed = 0;
        for key in keys {
            let block = NumHash::new(key.number(), key.hash());
            let pending_removed = self.pending.remove(key).is_some();
            self.canonical_pending.remove(key);
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
    /// Block number for this hash-indexed BAL.
    block_number: BlockNumber,
    /// Raw BAL payload.
    bal: RawBal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{RocksDBBuilder, RocksDBProvider};
    use alloy_primitives::B256;
    use tokio_stream::StreamExt;

    fn test_rocksdb(dir: &tempfile::TempDir) -> RocksDBProvider {
        RocksDBBuilder::new(dir.path())
            .with_table::<tables::BlockAccessLists>()
            .with_table::<tables::BlockAccessListBlockNumbers>()
            .build()
            .unwrap()
    }

    fn test_store() -> (tempfile::TempDir, RocksDBBalStore) {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb = test_rocksdb(&dir);
        (dir, RocksDBBalStore::new(rocksdb))
    }

    fn disk_bal(store: &RocksDBBalStore, block: NumHash) -> Option<Bytes> {
        store
            .rocksdb_provider()
            .get_raw::<tables::BlockAccessLists>(StoredBlockAccessListKey::new(
                block.number,
                block.hash,
            ))
            .unwrap()
            .map(|value| StoredBlockAccessList::decompress(&value).unwrap().into_raw())
    }

    fn read_many(store: &RocksDBBalStore, blocks: &[NumHash]) -> Vec<Option<Bytes>> {
        blocks.iter().map(|block| store.get_by_hash(block.hash).unwrap()).collect()
    }

    #[test]
    fn inserts_and_reads_by_hash() {
        let (_dir, store) = test_store();
        let hash = B256::random();
        let missing = NumHash::new(1, B256::random());
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(NumHash::new(1, hash), RawBal::from(bal.clone())).unwrap();

        assert_eq!(read_many(&store, &[NumHash::new(1, hash), missing]), vec![Some(bal), None]);
    }

    #[test]
    fn hash_lookup_reads_persisted_bal_through_store() {
        let (_dir, store) = test_store();
        let block = NumHash::new(1, B256::random());
        let bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(block, RawBal::from(bal.clone())).unwrap();
        store.flush(&[block]).unwrap();

        let store_with_empty_buffer = RocksDBBalStore::new(store.rocksdb_provider().clone());
        assert_eq!(store_with_empty_buffer.get_by_hash(block.hash).unwrap(), Some(bal));
    }

    #[test]
    fn flush_prunes_buffer_retention() {
        let (_dir, store) = test_store();
        let old = NumHash::new(1, B256::with_last_byte(1));
        let retained =
            NumHash::new(DEFAULT_BAL_BUFFER_RETENTION_DISTANCE + 2, B256::with_last_byte(2));
        let old_bal = Bytes::from_static(&[0xc1, 0x01]);
        let retained_bal = Bytes::from_static(&[0xc1, 0x02]);

        store.insert(old, RawBal::from(old_bal.clone())).unwrap();
        store.insert(retained, RawBal::from(retained_bal.clone())).unwrap();

        assert_eq!(
            store.get_by_hashes(&[old.hash, retained.hash]).unwrap(),
            vec![Some(old_bal), Some(retained_bal.clone())]
        );

        store.flush(&[retained]).unwrap();

        assert_eq!(
            store.get_by_hashes(&[old.hash, retained.hash]).unwrap(),
            vec![None, Some(retained_bal)]
        );
        assert_eq!(disk_bal(&store, old), None);
    }

    #[test]
    fn flush_prunes_only_durable_cache_entries() {
        let (_dir, store) = test_store();
        let old = NumHash::new(1, B256::with_last_byte(1));
        let tip = NumHash::new(DEFAULT_BAL_BUFFER_RETENTION_DISTANCE + 2, B256::with_last_byte(2));
        let old_bal = Bytes::from_static(&[0xc1, 0x01]);

        store.insert(old, RawBal::from(old_bal.clone())).unwrap();
        store.flush(&[old]).unwrap();
        store.flush(&[tip]).unwrap();

        assert!(!store.buffer.read().entries.contains_key(&old.hash));
        assert_eq!(disk_bal(&store, old), Some(old_bal.clone()));
        assert_eq!(store.get_by_hash(old.hash).unwrap(), Some(old_bal));
    }

    #[test]
    fn configured_buffer_retention_distance_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb = test_rocksdb(&dir);
        let store = RocksDBBalStore::with_buffer_retention_distance(rocksdb, 64);
        let old = NumHash::new(1, B256::with_last_byte(1));
        let tip = NumHash::new(34, B256::with_last_byte(2));

        store.insert(old, RawBal::from(Bytes::from_static(&[0xc1, 0x01]))).unwrap();
        store.flush(&[old]).unwrap();
        store.flush(&[tip]).unwrap();

        assert!(store.buffer.read().entries.contains_key(&old.hash));
    }

    #[test]
    fn flush_writes_only_requested_pending_bals() {
        let (_dir, store) = test_store();
        let block_1 = NumHash::new(1, B256::with_last_byte(1));
        let block_1_fork = NumHash::new(1, B256::with_last_byte(9));
        let block_2 = NumHash::new(2, B256::with_last_byte(2));
        let bal_1 = Bytes::from_static(&[0xc1, 0x01]);
        let bal_1_fork = Bytes::from_static(&[0xc1, 0x09]);
        let bal_2 = Bytes::from_static(&[0xc1, 0x02]);

        store.insert(block_1, RawBal::from(bal_1.clone())).unwrap();
        store.insert(block_1_fork, RawBal::from(bal_1_fork.clone())).unwrap();
        store.insert(block_2, RawBal::from(bal_2.clone())).unwrap();

        store.flush(&[block_1]).unwrap();

        assert_eq!(disk_bal(&store, block_1), Some(bal_1.clone()));
        assert_eq!(disk_bal(&store, block_1_fork), None);
        assert_eq!(disk_bal(&store, block_2), None);
        assert_eq!(
            store.get_by_hashes(&[block_1.hash, block_1_fork.hash, block_2.hash]).unwrap(),
            vec![Some(bal_1.clone()), Some(bal_1_fork), Some(bal_2)]
        );

        let store_with_empty_buffer = RocksDBBalStore::new(store.rocksdb_provider().clone());
        assert_eq!(
            store_with_empty_buffer
                .get_by_hashes(&[block_1.hash, block_1_fork.hash, block_2.hash])
                .unwrap(),
            vec![Some(bal_1), None, None]
        );
    }

    #[test]
    fn sparse_numbers_are_valid() {
        let (_dir, store) = test_store();
        let hash_a = B256::with_last_byte(1);
        let hash_b = B256::with_last_byte(2);
        let bal_a = Bytes::from_static(&[0xc1, 0x01]);
        let bal_b = Bytes::from_static(&[0xc1, 0x02]);

        let block_a = NumHash::new(2, hash_a);
        let block_b = NumHash::new(200, hash_b);

        store.insert(block_a, RawBal::from(bal_a.clone())).unwrap();
        store.flush(&[block_a]).unwrap();
        store.insert(block_b, RawBal::from(bal_b.clone())).unwrap();
        store.flush(&[block_b]).unwrap();

        assert_eq!(read_many(&store, &[block_a, block_b]), vec![Some(bal_a), Some(bal_b)]);
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
    fn prune_uses_eip_retention() {
        let (_dir, store) = test_store();
        let old_hash = B256::with_last_byte(1);
        let retained_hash = B256::with_last_byte(2);
        let retained_bal = Bytes::from_static(&[0xc1, 0x02]);
        let tip = BAL_RETENTION_PERIOD_SLOTS + 2;

        store
            .insert(NumHash::new(1, old_hash), RawBal::from(Bytes::from_static(&[0xc1, 0x01])))
            .unwrap();
        store.insert(NumHash::new(2, retained_hash), RawBal::from(retained_bal.clone())).unwrap();
        store.flush(&[NumHash::new(1, old_hash), NumHash::new(2, retained_hash)]).unwrap();

        assert_eq!(store.prune(tip).unwrap(), 1);
        assert_eq!(disk_bal(&store, NumHash::new(1, old_hash)), None);
        assert_eq!(disk_bal(&store, NumHash::new(2, retained_hash)), Some(retained_bal.clone()));
        assert_eq!(
            read_many(&store, &[NumHash::new(1, old_hash), NumHash::new(2, retained_hash)]),
            vec![None, Some(retained_bal)]
        );
    }

    #[test]
    fn stored_payload_hash_is_not_reverified() {
        let (_dir, store) = test_store();
        let block = NumHash::new(1, B256::with_last_byte(1));
        let key = StoredBlockAccessListKey::new(block.number, block.hash);
        let value = StoredBlockAccessList::new_unchecked(B256::ZERO, Bytes::from_static(&[0xc0]));

        store.rocksdb_provider().put::<tables::BlockAccessLists>(key, &value).unwrap();
        store
            .rocksdb_provider()
            .put::<tables::BlockAccessListBlockNumbers>(block.hash, &block.number)
            .unwrap();

        assert_eq!(store.get_by_hash(block.hash).unwrap(), Some(Bytes::from_static(&[0xc0])));
    }

    #[test]
    fn canonical_pending_entries_are_retried() {
        let mut buffer = RocksDBBalStoreBuffer::default();
        let first = NumHash::new(1, B256::with_last_byte(1));
        let second = NumHash::new(2, B256::with_last_byte(2));
        buffer.insert(first, RawBal::from(Bytes::from_static(&[0xc1, 0x01])));
        buffer.mark_canonical(&[first]);

        assert_eq!(buffer.canonical_pending_entries().len(), 1);

        // A failed flush leaves the canonical entry pending for the next attempt.
        buffer.insert(second, RawBal::from(Bytes::from_static(&[0xc1, 0x02])));
        buffer.mark_canonical(&[second]);
        let retry = buffer.canonical_pending_entries();

        assert_eq!(
            retry.iter().map(|(key, _)| NumHash::new(key.number(), key.hash())).collect::<Vec<_>>(),
            vec![first, second]
        );
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
