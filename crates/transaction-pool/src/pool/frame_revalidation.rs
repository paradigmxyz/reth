//! Cancellation-aware ownership of temporarily withdrawn frame transactions.

use crate::{PoolTransaction, ValidPoolTransaction};
use alloy_primitives::{Address, B256};
use std::{collections::HashMap, sync::Arc};

pub(super) struct FrameRevalidationQueue<T: PoolTransaction> {
    transactions: HashMap<B256, Arc<ValidPoolTransaction<T>>>,
    senders: HashMap<Address, B256>,
}

impl<T: PoolTransaction> Default for FrameRevalidationQueue<T> {
    fn default() -> Self {
        Self { transactions: HashMap::new(), senders: HashMap::new() }
    }
}

impl<T: PoolTransaction> FrameRevalidationQueue<T> {
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    pub(super) fn insert(&mut self, tx: Arc<ValidPoolTransaction<T>>) {
        let _ = self.cancel_sender(tx.sender());
        self.senders.insert(tx.sender(), *tx.hash());
        self.transactions.insert(*tx.hash(), tx);
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
        queue.insert(Arc::clone(&tx));
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
        queue.insert(Arc::clone(&old));
        let snapshot = queue.snapshot();

        queue.insert(Arc::clone(&new));

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
        queue.insert(Arc::clone(&old));
        queue.insert(Arc::clone(&new));

        assert!(!queue.take_current(&old));
        assert!(queue.take_current(&new));
        assert!(queue.is_empty());
    }

    #[test]
    fn successful_take_removes_transaction_once() {
        let mut factory = MockTransactionFactory::default();
        let tx = transaction(&mut factory, Address::repeat_byte(4), B256::repeat_byte(4), 0);
        let mut queue = FrameRevalidationQueue::default();
        queue.insert(Arc::clone(&tx));

        assert!(queue.take_current(&tx));
        assert!(!queue.take_current(&tx));
        assert!(queue.is_empty());
    }
}
