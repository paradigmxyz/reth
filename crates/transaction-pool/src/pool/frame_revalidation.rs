//! Cancellation-aware ownership of temporarily withdrawn frame transactions.

use crate::{PoolTransaction, SubPoolLimit, ValidPoolTransaction};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    Address, B256, U256,
};
use std::sync::Arc;

pub(super) struct FrameRevalidationQueue<T: PoolTransaction> {
    transactions: B256Map<Arc<ValidPoolTransaction<T>>>,
    senders: AddressMap<B256>,
}

impl<T: PoolTransaction> Default for FrameRevalidationQueue<T> {
    fn default() -> Self {
        Self { transactions: Default::default(), senders: Default::default() }
    }
}

impl<T: PoolTransaction> FrameRevalidationQueue<T> {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// Returns superseded ownership for cleanup, excluding refreshes of the same hash.
    pub(super) fn insert(
        &mut self,
        tx: Arc<ValidPoolTransaction<T>>,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let superseded = self.cancel_sender(tx.sender()).filter(|old| old.hash() != tx.hash());
        self.senders.insert(tx.sender(), *tx.hash());
        self.transactions.insert(*tx.hash(), tx);
        superseded
    }

    pub(super) fn cancel(&mut self, hash: &B256) {
        let _ = self.remove(hash);
    }

    pub(super) fn remove(&mut self, hash: &B256) -> Option<Arc<ValidPoolTransaction<T>>> {
        let tx = self.transactions.remove(hash)?;
        self.senders.remove(&tx.sender());
        Some(tx)
    }

    pub(super) fn cancel_sender(
        &mut self,
        sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let hash = self.senders.remove(&sender)?;
        self.transactions.remove(&hash)
    }

    pub(super) fn contains_sender(&self, sender: Address) -> bool {
        self.senders.contains_key(&sender)
    }

    pub(super) fn cancel_replaced(
        &mut self,
        sender: Address,
        nonce: u64,
        keys: Option<&[U256]>,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let hash = *self.senders.get(&sender)?;
        let old = self.transactions.get(&hash)?;
        if old.nonce() != nonce || old.transaction.eip8250_nonce_keys() != keys {
            return None;
        }
        self.remove(&hash)
    }

    pub(super) fn snapshot(&self) -> Vec<Arc<ValidPoolTransaction<T>>> {
        self.transactions.values().cloned().collect()
    }

    pub(super) fn take_current(&mut self, tx: &Arc<ValidPoolTransaction<T>>) -> bool {
        if self.transactions.get(tx.hash()).is_some_and(|current| Arc::ptr_eq(current, tx)) {
            self.cancel(tx.hash());
            true
        } else {
            false
        }
    }

    pub(super) fn discard_worst(
        &mut self,
        limit: SubPoolLimit,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let mut size = self
            .transactions
            .values()
            .fold(0usize, |size, tx| size.saturating_add(tx.transaction.size()));
        if !limit.is_exceeded(self.transactions.len(), size) {
            return Vec::new()
        }
        let mut candidates = self.snapshot();
        candidates.sort_unstable_by_key(|tx| (tx.timestamp, *tx.hash()));
        let mut discarded = Vec::new();
        for candidate in candidates {
            if !limit.is_exceeded(self.transactions.len(), size) {
                break
            }
            if let Some(tx) = self.remove(candidate.hash()) {
                size = size.saturating_sub(tx.transaction.size());
                discarded.push(tx);
            }
        }
        discarded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockTransaction, MockTransactionFactory};

    fn transaction(
        factory: &mut MockTransactionFactory,
        sender: Address,
        hash: B256,
        nonce: u64,
    ) -> Arc<ValidPoolTransaction<MockTransaction>> {
        factory.validated_arc(
            MockTransaction::legacy().with_sender(sender).with_hash(hash).with_nonce(nonce),
        )
    }

    #[test]
    fn cancelled_snapshot_cannot_be_restored() {
        let mut factory = MockTransactionFactory::default();
        let tx = transaction(&mut factory, Address::repeat_byte(1), B256::repeat_byte(1), 0);
        let mut queue = FrameRevalidationQueue::default();
        assert!(queue.insert(Arc::clone(&tx)).is_none());
        let snapshot = queue.snapshot();

        queue.cancel(tx.hash());

        assert!(!queue.take_current(&snapshot[0]));
        assert!(queue.is_empty());
    }

    #[test]
    fn replacing_sender_cancels_stale_snapshot() {
        let mut factory = MockTransactionFactory::default();
        let sender = Address::repeat_byte(2);
        let old = transaction(&mut factory, sender, B256::repeat_byte(1), 0);
        let new = transaction(&mut factory, sender, B256::repeat_byte(2), 1);
        let mut queue = FrameRevalidationQueue::default();
        assert!(queue.insert(Arc::clone(&old)).is_none());
        let snapshot = queue.snapshot();

        let superseded = queue.insert(Arc::clone(&new)).unwrap();
        assert!(Arc::ptr_eq(&superseded, &old));

        assert!(!queue.take_current(&snapshot[0]));
        assert!(queue.take_current(&new));
        assert!(queue.is_empty());
    }

    #[test]
    fn identical_hash_with_new_arc_cannot_be_taken_via_stale_arc() {
        let mut factory = MockTransactionFactory::default();
        let sender = Address::repeat_byte(3);
        let hash = B256::repeat_byte(3);
        let old = transaction(&mut factory, sender, hash, 0);
        let new = transaction(&mut factory, sender, hash, 0);
        let mut queue = FrameRevalidationQueue::default();
        assert!(queue.insert(Arc::clone(&old)).is_none());
        assert!(queue.insert(Arc::clone(&new)).is_none());

        assert!(!queue.take_current(&old));
        assert!(queue.take_current(&new));
        assert!(queue.is_empty());
    }

    #[test]
    fn successful_take_removes_transaction_once() {
        let mut factory = MockTransactionFactory::default();
        let tx = transaction(&mut factory, Address::repeat_byte(4), B256::repeat_byte(4), 0);
        let mut queue = FrameRevalidationQueue::default();
        assert!(queue.insert(Arc::clone(&tx)).is_none());

        assert!(queue.take_current(&tx));
        assert!(!queue.take_current(&tx));
        assert!(queue.is_empty());
    }

    #[test]
    fn retry_queue_limits_cancel_oldest_snapshots() {
        let mut factory = MockTransactionFactory::default();
        let mut first = transaction(&mut factory, Address::repeat_byte(1), B256::repeat_byte(1), 0);
        let mut second =
            transaction(&mut factory, Address::repeat_byte(2), B256::repeat_byte(2), 0);
        Arc::make_mut(&mut first).transaction.set_size(100);
        Arc::make_mut(&mut second).transaction.set_size(100);
        Arc::make_mut(&mut second).timestamp = first.timestamp + std::time::Duration::from_secs(1);
        let mut queue = FrameRevalidationQueue::default();
        assert!(queue.insert(Arc::clone(&first)).is_none());
        assert!(queue.insert(Arc::clone(&second)).is_none());

        let discarded = queue.discard_worst(SubPoolLimit::new(1, usize::MAX));
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].hash(), first.hash());
        assert!(!queue.take_current(&first));
        assert!(!queue.contains_sender(first.sender()));
        assert!(queue.contains_sender(second.sender()));

        let discarded = queue.discard_worst(SubPoolLimit::new(usize::MAX, 99));
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].hash(), second.hash());
        assert!(!queue.take_current(&second));
        assert!(queue.is_empty());
    }
}
