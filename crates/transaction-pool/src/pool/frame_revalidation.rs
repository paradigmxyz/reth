//! Cancellation-aware ownership of temporarily withdrawn frame transactions.

use crate::{PoolTransaction, ValidPoolTransaction};
use alloy_primitives::{Address, B256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub(super) struct FrameRevalidationQueue<T: PoolTransaction> {
    transactions: HashMap<B256, Arc<ValidPoolTransaction<T>>>,
    senders: HashMap<Address, BTreeMap<u64, B256>>,
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
        let _ = self.cancel_nonce(tx.sender(), tx.nonce());
        self.senders.entry(tx.sender()).or_default().insert(tx.nonce(), *tx.hash());
        self.transactions.insert(*tx.hash(), tx);
    }

    pub(super) fn cancel(&mut self, hash: &B256) {
        let _ = self.remove(hash);
    }

    pub(super) fn remove(&mut self, hash: &B256) -> Option<Arc<ValidPoolTransaction<T>>> {
        let tx = self.transactions.remove(hash)?;
        let nonces = self.senders.get_mut(&tx.sender())?;
        nonces.remove(&tx.nonce());
        if nonces.is_empty() {
            self.senders.remove(&tx.sender());
        }
        Some(tx)
    }

    pub(super) fn get(&self, sender: Address, nonce: u64) -> Option<&Arc<ValidPoolTransaction<T>>> {
        let hash = self.senders.get(&sender)?.get(&nonce)?;
        self.transactions.get(hash)
    }

    pub(super) fn cancel_nonce(
        &mut self,
        sender: Address,
        nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let hash = *self.senders.get(&sender)?.get(&nonce)?;
        self.remove(&hash)
    }

    pub(super) fn cancel_sender(&mut self, sender: Address) -> Vec<Arc<ValidPoolTransaction<T>>> {
        self.cancel_descendants(sender, 0)
    }

    pub(super) fn cancel_descendants(
        &mut self,
        sender: Address,
        nonce: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let hashes: Vec<_> = self
            .senders
            .get(&sender)
            .into_iter()
            .flat_map(|nonces| nonces.range(nonce..).map(|(_, hash)| *hash))
            .collect();
        hashes.into_iter().filter_map(|hash| self.remove(&hash)).collect()
    }

    pub(super) fn snapshot(&self) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let mut transactions: Vec<_> = self.transactions.values().cloned().collect();
        // Restore ancestors before their descendants, including blob transactions whose
        // admission requires a consecutive nonce chain.
        transactions.sort_unstable_by_key(|tx| *tx.id());
        transactions
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
    fn replacing_nonce_cancels_stale_snapshot() {
        let mut factory = MockTransactionFactory::default();
        let sender = Address::repeat_byte(2);
        let old = transaction(&mut factory, sender, B256::repeat_byte(1), 0);
        let new = transaction(&mut factory, sender, B256::repeat_byte(2), 0);
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

    #[test]
    fn different_nonces_survive_withdrawal_and_restore_in_order() {
        let mut factory = MockTransactionFactory::default();
        let sender = Address::repeat_byte(5);
        let first = transaction(&mut factory, sender, B256::repeat_byte(1), 0);
        let second = transaction(&mut factory, sender, B256::repeat_byte(2), 1);
        let mut queue = FrameRevalidationQueue::default();
        queue.insert(Arc::clone(&second));
        queue.insert(Arc::clone(&first));

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].nonce(), 0);
        assert_eq!(snapshot[1].nonce(), 1);
        assert!(queue.take_current(&first));
        assert!(queue.take_current(&second));
        assert!(queue.is_empty());
    }

    #[test]
    fn cancellation_preserves_lower_nonces_and_other_senders() {
        let mut factory = MockTransactionFactory::default();
        let sender = Address::repeat_byte(6);
        let first = transaction(&mut factory, sender, B256::repeat_byte(1), 0);
        let second = transaction(&mut factory, sender, B256::repeat_byte(2), 1);
        let other = transaction(&mut factory, Address::repeat_byte(7), B256::repeat_byte(3), 0);
        let mut queue = FrameRevalidationQueue::default();
        for tx in [&first, &second, &other] {
            queue.insert(Arc::clone(tx));
        }

        assert_eq!(queue.cancel_descendants(sender, 1).len(), 1);
        assert!(!queue.take_current(&second));
        assert!(queue.get(sender, 0).is_some());
        assert_eq!(queue.cancel_sender(sender).len(), 1);
        assert!(!queue.take_current(&first));
        assert!(queue.take_current(&other));
        assert!(queue.is_empty());
    }
}
