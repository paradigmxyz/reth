use crate::{
    identifier::TransactionId,
    pool::{best::BestTransactions, size::SizeTracker},
    Priority, TransactionOrdering, ValidPoolTransaction,
};

use crate::pool::best::BestTransactionsWithBasefee;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::sync::broadcast;

/// A pool of validated and gapless transactions that are ready to be executed on the current state
/// and are waiting to be included in a block.
///
/// This pool distinguishes between `independent` transactions and pending transactions. A
/// transaction is `independent`, if it is in the pending pool, and it has the current on chain
/// nonce of the sender. Meaning `independent` transactions can be executed right away, other
/// pending transactions depend on at least one `independent` transaction.
///
/// Once an `independent` transaction was executed it *unlocks* the next nonce, if this transaction
/// is also pending, then this will be moved to the `independent` queue.
#[derive(Clone)]
pub(crate) struct PendingPool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: T,
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions were submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, PendingTransaction<T>>,
    /// _All_ transactions sorted by priority
    all: BTreeSet<PendingTransaction<T>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PendingTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`](crate::traits::PoolTransaction::size).
    size_of: SizeTracker,
    /// Used to broadcast new transactions that have been added to the PendingPool to existing
    /// snapshots of this pool.
    new_transaction_notifier: broadcast::Sender<PendingTransaction<T>>,
}

// === impl PendingPool ===

impl<T: TransactionOrdering> PendingPool<T> {
    /// Create a new pool instance.
    pub(crate) fn new(ordering: T) -> Self {
        let (new_transaction_notifier, _) = broadcast::channel(200);
        Self {
            ordering,
            submission_id: 0,
            by_id: Default::default(),
            all: Default::default(),
            independent_transactions: Default::default(),
            size_of: Default::default(),
            new_transaction_notifier,
        }
    }

    /// Clear all transactions from the pool without resetting other values.
    /// Used for atomic reordering during basefee update.
    ///
    /// # Returns
    ///
    /// Returns all transactions by id.
    fn clear_transactions(&mut self) -> BTreeMap<TransactionId, PendingTransaction<T>> {
        self.independent_transactions.clear();
        self.all.clear();
        self.size_of.reset();
        std::mem::take(&mut self.by_id)
    }

    /// Returns an iterator over all transactions that are _currently_ ready.
    ///
    /// 1. The iterator _always_ returns transaction in order: It never returns a transaction with
    /// an unsatisfied dependency and only returns them if dependency transaction were yielded
    /// previously. In other words: The nonces of transactions with the same sender will _always_
    /// increase by exactly 1.
    ///
    /// The order of transactions which satisfy (1.) is determent by their computed priority: A
    /// transaction with a higher priority is returned before a transaction with a lower priority.
    ///
    /// If two transactions have the same priority score, then the transactions which spent more
    /// time in pool (were added earlier) are returned first.
    ///
    /// NOTE: while this iterator returns transaction that pool considers valid at this point, they
    /// could potentially be become invalid at point of execution. Therefore, this iterator
    /// provides a way to mark transactions that the consumer of this iterator considers invalid. In
    /// which case the transaction's subgraph is also automatically marked invalid, See (1.).
    /// Invalid transactions are skipped.
    pub(crate) fn best(&self) -> BestTransactions<T> {
        BestTransactions {
            all: self.by_id.clone(),
            independent: self.independent_transactions.clone(),
            invalid: Default::default(),
            new_transaction_receiver: Some(self.new_transaction_notifier.subscribe()),
            skip_blobs: false,
        }
    }

    /// Same as `best` but only returns transactions that satisfy the given basefee.
    pub(crate) fn best_with_basefee(&self, base_fee: u64) -> BestTransactionsWithBasefee<T> {
        BestTransactionsWithBasefee { best: self.best(), base_fee }
    }

    /// Same as `best` but also includes the given unlocked transactions.
    ///
    /// This mimics the [Self::add_transaction] method, but does not insert the transactions into
    /// pool but only into the returned iterator.
    ///
    /// Note: this does not insert the unlocked transactions into the pool.
    ///
    /// # Panics
    ///
    /// if the transaction is already included
    pub(crate) fn best_with_unlocked(
        &self,
        unlocked: Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
        base_fee: u64,
    ) -> BestTransactions<T> {
        let mut best = self.best();
        let mut submission_id = self.submission_id;
        for tx in unlocked {
            submission_id += 1;
            debug_assert!(!best.all.contains_key(tx.id()), "transaction already included");
            let priority = self.ordering.priority(&tx.transaction, base_fee);
            let tx_id = *tx.id();
            let transaction = PendingTransaction { submission_id, transaction: tx, priority };
            if best.ancestor(&tx_id).is_none() {
                best.independent.insert(transaction.clone());
            }
            best.all.insert(tx_id, transaction);
        }

        best
    }

    /// Returns an iterator over all transactions in the pool
    pub(crate) fn all(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.by_id.values().map(|tx| tx.transaction.clone())
    }

    /// Updates the pool with the new base fee. Reorders transactions by new priorities. Removes
    /// from the subpool all transactions and their dependents that no longer satisfy the given
    /// base fee (`tx.fee < base_fee`).
    ///
    /// Note: the transactions are not returned in a particular order.
    ///
    /// # Returns
    ///
    /// Removed transactions that no longer satisfy the base fee.
    pub(crate) fn update_base_fee(
        &mut self,
        base_fee: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        // Create a collection for removed transactions.
        let mut removed = Vec::new();

        // Drain and iterate over all transactions.
        let mut transactions_iter = self.clear_transactions().into_iter().peekable();
        while let Some((id, mut tx)) = transactions_iter.next() {
            if tx.transaction.max_fee_per_gas() < base_fee as u128 {
                // Add this tx to the removed collection since it no longer satisfies the base fee
                // condition. Decrease the total pool size.
                removed.push(Arc::clone(&tx.transaction));

                // Remove all dependent transactions.
                'this: while let Some((next_id, next_tx)) = transactions_iter.peek() {
                    if next_id.sender != id.sender {
                        break 'this
                    }
                    removed.push(Arc::clone(&next_tx.transaction));
                    transactions_iter.next();
                }
            } else {
                // Re-insert the transaction with new priority.
                tx.priority = self.ordering.priority(&tx.transaction.transaction, base_fee);

                self.size_of += tx.transaction.size();
                if self.ancestor(&id).is_none() {
                    self.independent_transactions.insert(tx.clone());
                }
                self.all.insert(tx.clone());
                self.by_id.insert(id, tx);
            }
        }

        removed
    }

    /// Returns the ancestor the given transaction, the transaction with `nonce - 1`.
    ///
    /// Note: for a transaction with nonce higher than the current on chain nonce this will always
    /// return an ancestor since all transaction in this pool are gapless.
    fn ancestor(&self, id: &TransactionId) -> Option<&PendingTransaction<T>> {
        self.by_id.get(&id.unchecked_ancestor()?)
    }

    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// if the transaction is already included
    pub(crate) fn add_transaction(
        &mut self,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
        base_fee: u64,
    ) {
        assert!(
            !self.by_id.contains_key(tx.id()),
            "transaction already included {:?}",
            self.by_id.contains_key(tx.id())
        );

        // keep track of size
        self.size_of += tx.size();

        let tx_id = *tx.id();

        let submission_id = self.next_id();
        let priority = self.ordering.priority(&tx.transaction, base_fee);
        let tx = PendingTransaction { submission_id, transaction: tx, priority };

        // If there's __no__ ancestor in the pool, then this transaction is independent, this is
        // guaranteed because this pool is gapless.
        if self.ancestor(&tx_id).is_none() {
            self.independent_transactions.insert(tx.clone());
        }
        self.all.insert(tx.clone());

        // send the new transaction to any existing pendingpool snapshot iterators
        if self.new_transaction_notifier.receiver_count() > 0 {
            let _ = self.new_transaction_notifier.send(tx.clone());
        }

        self.by_id.insert(tx_id, tx);
    }

    /// Removes a _mined_ transaction from the pool.
    ///
    /// If the transaction has a descendant transaction it will advance it to the best queue.
    pub(crate) fn prune_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // mark the next as independent if it exists
        if let Some(unlocked) = self.by_id.get(&id.descendant()) {
            self.independent_transactions.insert(unlocked.clone());
        };
        self.remove_transaction(id)
    }

    /// Removes the transaction from the pool.
    ///
    /// Note: this only removes the given transaction.
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let tx = self.by_id.remove(id)?;
        self.size_of -= tx.transaction.size();
        self.all.remove(&tx);
        self.independent_transactions.remove(&tx);
        Some(tx.transaction)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// Removes the worst transaction from this pool.
    pub(crate) fn pop_worst(&mut self) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let worst = self.all.iter().next().map(|tx| *tx.transaction.id())?;
        self.remove_transaction(&worst)
    }

    /// The reported size of all transactions in this pool.
    pub(crate) fn size(&self) -> usize {
        self.size_of.into()
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the pool is empty
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Returns `true` if the transaction with the given id is already included in this pool.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: &TransactionId) -> bool {
        self.by_id.contains_key(id)
    }
}

/// A transaction that is ready to be included in a block.
pub(crate) struct PendingTransaction<T: TransactionOrdering> {
    /// Identifier that tags when transaction was submitted in the pool.
    pub(crate) submission_id: u64,
    /// Actual transaction.
    pub(crate) transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// The priority value assigned by the used `Ordering` function.
    pub(crate) priority: Priority<T::PriorityValue>,
}

impl<T: TransactionOrdering> PendingTransaction<T> {
    /// The next transaction of the sender: `nonce + 1`
    pub(crate) fn unlocks(&self) -> TransactionId {
        self.transaction.transaction_id.descendant()
    }
}

impl<T: TransactionOrdering> Clone for PendingTransaction<T> {
    fn clone(&self) -> Self {
        Self {
            submission_id: self.submission_id,
            transaction: Arc::clone(&self.transaction),
            priority: self.priority.clone(),
        }
    }
}

impl<T: TransactionOrdering> Eq for PendingTransaction<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for PendingTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for PendingTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for PendingTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `submission_id`. This ensures that transactions with same priority are not
        // equal, so they're not replaced in the set
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
        PoolTransaction,
    };

    #[test]
    fn test_enforce_basefee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx.clone(), 0);

        assert!(pool.by_id.contains_key(tx.id()));
        assert_eq!(pool.len(), 1);

        let removed = pool.update_base_fee(0);
        assert!(removed.is_empty());

        let removed = pool.update_base_fee((tx.max_fee_per_gas() + 1) as u64);
        assert_eq!(removed.len(), 1);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_enforce_basefee_descendant() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());
        let t = MockTransaction::eip1559().inc_price_by(10);
        let root_tx = f.validated_arc(t.clone());
        pool.add_transaction(root_tx.clone(), 0);

        let descendant_tx = f.validated_arc(t.inc_nonce().decr_price());
        pool.add_transaction(descendant_tx.clone(), 0);

        assert!(pool.by_id.contains_key(root_tx.id()));
        assert!(pool.by_id.contains_key(descendant_tx.id()));
        assert_eq!(pool.len(), 2);

        assert_eq!(pool.independent_transactions.len(), 1);

        let removed = pool.update_base_fee(0);
        assert!(removed.is_empty());

        // two dependent tx in the pool with decreasing fee

        {
            let mut pool2 = pool.clone();
            let removed = pool2.update_base_fee((descendant_tx.max_fee_per_gas() + 1) as u64);
            assert_eq!(removed.len(), 1);
            assert_eq!(pool2.len(), 1);
            // descendant got popped
            assert!(pool2.by_id.contains_key(root_tx.id()));
            assert!(!pool2.by_id.contains_key(descendant_tx.id()));
        }

        // remove root transaction via fee
        let removed = pool.update_base_fee((root_tx.max_fee_per_gas() + 1) as u64);
        assert_eq!(removed.len(), 2);
        assert!(pool.is_empty());
    }

    #[test]
    fn evict_worst() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        let t = MockTransaction::eip1559();
        pool.add_transaction(f.validated_arc(t.clone()), 0);

        let t2 = MockTransaction::eip1559().inc_price_by(10);
        pool.add_transaction(f.validated_arc(t2), 0);

        // First transaction should be evicted.
        assert_eq!(pool.pop_worst().map(|tx| *tx.hash()), Some(*t.hash()));
    }
}
