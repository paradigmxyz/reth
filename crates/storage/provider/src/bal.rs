use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use alloy_eips::NumHash;
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_prune_types::PruneMode;
use reth_storage_api::{
    BalNotification, BalNotificationStream, BalStore, GetBlockAccessListLimit, SealedBal,
};
use reth_storage_errors::provider::ProviderResult;
use reth_tokio_util::EventSender;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

/// Basic in-memory BAL store keyed by block hash.
#[derive(Debug, Clone)]
pub struct InMemoryBalStore {
    config: BalConfig,
    inner: Arc<RwLock<InMemoryBalStoreInner>>,
    notifications: EventSender<BalNotification>,
}

impl InMemoryBalStore {
    /// Creates a new in-memory BAL store with the given config.
    pub fn new(config: BalConfig) -> Self {
        let notifications = EventSender::new(DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE);
        Self {
            config,
            inner: Arc::new(RwLock::new(InMemoryBalStoreInner::default())),
            notifications,
        }
    }
}

// Match the canonical state broadcast buffer so BAL subscriptions behave like the existing
// in-memory notification path. This is a bounded best-effort channel, not a durability boundary.
const DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE: usize = 256;

impl Default for InMemoryBalStore {
    fn default() -> Self {
        Self::new(BalConfig::default())
    }
}

/// Configuration for BAL storage.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BalConfig {
    /// Retention policy for BALs kept in memory.
    in_memory_retention: Option<PruneMode>,
}

impl BalConfig {
    /// Returns a config with no in-memory BAL retention limit.
    pub const fn unbounded() -> Self {
        Self { in_memory_retention: None }
    }

    /// Returns a config with the given in-memory BAL retention policy.
    pub const fn with_in_memory_retention(in_memory_retention: PruneMode) -> Self {
        Self { in_memory_retention: Some(in_memory_retention) }
    }
}

impl Default for BalConfig {
    fn default() -> Self {
        Self::with_in_memory_retention(PruneMode::Distance(BAL_RETENTION_PERIOD_SLOTS))
    }
}

#[derive(Debug, Default)]
struct InMemoryBalStoreInner {
    entries: HashMap<BlockHash, BalEntry>,
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    highest_block_number: Option<BlockNumber>,
}

impl InMemoryBalStoreInner {
    // Inserts a BAL and keeps the block-number index in sync.
    fn insert(&mut self, block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) {
        let empty_block_number =
            self.entries.insert(block_hash, BalEntry { block_number, bal }).and_then(|entry| {
                let hashes = self.hashes_by_number.get_mut(&entry.block_number)?;
                hashes.retain(|hash| *hash != block_hash);
                hashes.is_empty().then_some(entry.block_number)
            });

        if let Some(block_number) = empty_block_number {
            self.hashes_by_number.remove(&block_number);
        }

        self.hashes_by_number.entry(block_number).or_default().push(block_hash);
        self.highest_block_number = Some(
            self.highest_block_number.map_or(block_number, |highest| highest.max(block_number)),
        );
    }

    // Removes BALs outside the configured retention window.
    fn prune(&mut self, prune_mode: Option<PruneMode>) {
        let Some(prune_mode) = prune_mode else { return };
        let Some(tip) = self.highest_block_number else { return };

        while let Some((&block_number, _)) = self.hashes_by_number.first_key_value() {
            if !prune_mode.should_prune(block_number, tip) {
                break
            }

            let Some((_, hashes)) = self.hashes_by_number.pop_first() else { break };
            for hash in hashes {
                self.entries.remove(&hash);
            }
        }
    }
}

#[derive(Debug)]
struct BalEntry {
    block_number: BlockNumber,
    bal: Bytes,
}

impl BalStore for InMemoryBalStore {
    fn insert(&self, num_hash: NumHash, bal: SealedBal) -> ProviderResult<()> {
        let mut inner = self.inner.write();
        inner.insert(num_hash.hash, num_hash.number, bal.clone_inner());
        inner.prune(self.config.in_memory_retention);
        self.notifications.notify(BalNotification::new(num_hash, bal));
        Ok(())
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let inner = self.inner.read();
        let mut result = Vec::with_capacity(block_hashes.len());

        for hash in block_hashes {
            result.push(inner.entries.get(hash).map(|entry| entry.bal.clone()));
        }

        Ok(result)
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let inner = self.inner.read();
        let mut size = 0;

        for hash in block_hashes {
            let bal = inner
                .entries
                .get(hash)
                .map(|entry| entry.bal.clone())
                .unwrap_or_else(|| Bytes::from_static(&[0xc0]));
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }

        Ok(())
    }

    fn get_by_range(&self, _start: BlockNumber, _count: u64) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::new())
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.notifications.new_listener()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Sealed, B256};
    use tokio_stream::StreamExt;

    fn sealed_bal(bal: Bytes) -> SealedBal {
        Sealed::new_unchecked(bal.clone(), keccak256(&bal))
    }

    #[test]
    fn insert_and_lookup_by_hash() {
        let store = InMemoryBalStore::default();
        let hash = B256::random();
        let missing = B256::random();
        let bal = Bytes::from_static(b"bal");

        store.insert(NumHash::new(1, hash), sealed_bal(bal.clone())).unwrap();

        assert_eq!(store.get_by_hashes(&[hash, missing]).unwrap(), vec![Some(bal), None]);
    }

    #[test]
    fn range_lookup_is_empty() {
        let store = InMemoryBalStore::default();

        assert!(store.get_by_range(1, 10).unwrap().is_empty());
    }

    #[test]
    fn limited_lookup_returns_prefix() {
        let store = InMemoryBalStore::default();
        let hash0 = B256::random();
        let hash1 = B256::random();
        let hash2 = B256::random();
        let bal0 = Bytes::from_static(&[0xc1, 0x01]);
        let bal1 = Bytes::from_static(&[0xc1, 0x02]);
        let bal2 = Bytes::from_static(&[0xc1, 0x03]);

        store.insert(NumHash::new(1, hash0), sealed_bal(bal0.clone())).unwrap();
        store.insert(NumHash::new(2, hash1), sealed_bal(bal1.clone())).unwrap();
        store.insert(NumHash::new(3, hash2), sealed_bal(bal2)).unwrap();

        let limited = store
            .get_by_hashes_with_limit(
                &[hash0, hash1, hash2],
                GetBlockAccessListLimit::ResponseSizeSoftLimit(2),
            )
            .unwrap();

        assert_eq!(limited, vec![bal0, bal1]);
    }

    #[test]
    fn default_retention_prunes_old_bals() {
        let store = InMemoryBalStore::default();
        let old_hash = B256::random();
        let retained_hash = B256::random();
        let tip_hash = B256::random();
        let old_bal = Bytes::from_static(b"old");
        let retained_bal = Bytes::from_static(b"retained");
        let tip_bal = Bytes::from_static(b"tip");

        store.insert(NumHash::new(1, old_hash), sealed_bal(old_bal)).unwrap();
        store
            .insert(
                NumHash::new(BAL_RETENTION_PERIOD_SLOTS, retained_hash),
                sealed_bal(retained_bal.clone()),
            )
            .unwrap();
        store
            .insert(
                NumHash::new(BAL_RETENTION_PERIOD_SLOTS + 2, tip_hash),
                sealed_bal(tip_bal.clone()),
            )
            .unwrap();

        assert_eq!(
            store.get_by_hashes(&[old_hash, retained_hash, tip_hash]).unwrap(),
            vec![None, Some(retained_bal), Some(tip_bal)]
        );
    }

    #[test]
    fn unbounded_retention_keeps_old_bals() {
        let store = InMemoryBalStore::new(BalConfig::unbounded());
        let old_hash = B256::random();
        let tip_hash = B256::random();
        let old_bal = Bytes::from_static(b"old");
        let tip_bal = Bytes::from_static(b"tip");

        store.insert(NumHash::new(1, old_hash), sealed_bal(old_bal.clone())).unwrap();
        store
            .insert(
                NumHash::new(BAL_RETENTION_PERIOD_SLOTS + 1, tip_hash),
                sealed_bal(tip_bal.clone()),
            )
            .unwrap();

        assert_eq!(
            store.get_by_hashes(&[old_hash, tip_hash]).unwrap(),
            vec![Some(old_bal), Some(tip_bal)]
        );
    }

    #[test]
    fn reinserting_hash_updates_number_index() {
        let store =
            InMemoryBalStore::new(BalConfig::with_in_memory_retention(PruneMode::Before(2)));
        let hash = B256::random();
        let bal = Bytes::from_static(b"bal");

        store.insert(NumHash::new(1, hash), sealed_bal(Bytes::from_static(b"old"))).unwrap();
        store.insert(NumHash::new(2, hash), sealed_bal(bal.clone())).unwrap();

        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
    }

    #[tokio::test]
    async fn insert_notifies_subscribers() {
        let store = InMemoryBalStore::default();
        let hash = B256::random();
        let block_number = 7;
        let bal = Bytes::from_static(b"bal");
        let mut stream = store.bal_stream();

        let sealed_bal = sealed_bal(bal);

        store.insert(NumHash::new(block_number, hash), sealed_bal.clone()).unwrap();

        assert_eq!(
            stream.next().await.unwrap(),
            BalNotification::new(NumHash::new(block_number, hash), sealed_bal)
        );
    }

    #[test]
    fn insert_without_subscribers_still_succeeds() {
        let store = InMemoryBalStore::default();

        assert!(store
            .insert(NumHash::new(1, B256::random()), sealed_bal(Bytes::from_static(b"bal")))
            .is_ok());
    }

    #[tokio::test]
    async fn bal_stream_skips_lagged_notifications() {
        let store = InMemoryBalStore::new(BalConfig::unbounded());
        let mut stream = store.bal_stream();

        for number in 0..=DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE as u64 {
            store
                .insert(
                    NumHash::new(number, B256::random()),
                    sealed_bal(Bytes::from(vec![number as u8])),
                )
                .unwrap();
        }

        let first = stream.next().await.unwrap();
        let second = stream.next().await.unwrap();

        assert_eq!(first.num_hash.number, 1);
        assert_eq!(second.num_hash.number, 2);
    }

    #[tokio::test]
    async fn cloned_store_shares_notification_channel() {
        let store = InMemoryBalStore::default();
        let clone = store.clone();
        let hash = B256::random();
        let block_number = 9;
        let bal = Bytes::from_static(b"bal");
        let mut stream = clone.bal_stream();

        let sealed_bal = sealed_bal(bal);

        store.insert(NumHash::new(block_number, hash), sealed_bal.clone()).unwrap();

        assert_eq!(
            stream.next().await.unwrap(),
            BalNotification::new(NumHash::new(block_number, hash), sealed_bal)
        );
    }
}
