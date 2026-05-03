use alloy_eips::NumHash;
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_storage_api::{
    BalNotification, BalNotificationStream, BalStore, GetBlockAccessListLimit, SealedBal,
};
use reth_storage_errors::provider::ProviderResult;
use reth_tokio_util::EventSender;
use std::{collections::HashMap, sync::Arc};

/// Basic in-memory BAL store keyed by block hash.
#[derive(Debug, Clone)]
pub struct InMemoryBalStore {
    entries: Arc<RwLock<HashMap<BlockHash, Bytes>>>,
    notifications: EventSender<BalNotification>,
}

// Match the canonical state broadcast buffer so BAL subscriptions behave like the existing
// in-memory notification path. This is a bounded best-effort channel, not a durability boundary.
const DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE: usize = 256;

impl Default for InMemoryBalStore {
    fn default() -> Self {
        let notifications = EventSender::new(DEFAULT_BAL_NOTIFICATION_CHANNEL_SIZE);
        Self { entries: Default::default(), notifications }
    }
}

impl BalStore for InMemoryBalStore {
    fn insert(&self, num_hash: NumHash, bal: SealedBal) -> ProviderResult<()> {
        self.entries.write().insert(num_hash.hash, bal.clone_inner());
        self.notifications.notify(BalNotification::new(num_hash, bal));
        Ok(())
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let entries = self.entries.read();
        let mut result = Vec::with_capacity(block_hashes.len());

        for hash in block_hashes {
            result.push(entries.get(hash).cloned());
        }

        Ok(result)
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let entries = self.entries.read();
        let mut size = 0;

        for hash in block_hashes {
            let bal = entries.get(hash).cloned().unwrap_or_else(|| Bytes::from_static(&[0xc0]));
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
        let store = InMemoryBalStore::default();
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
