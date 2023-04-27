use crate::{
    identifier::TransactionId,
    pool::{best::BestTransactions, size::SizeTracker},
    TransactionOrdering, ValidPoolTransaction,
};

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
pub(crate) struct PendingPool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: T,
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, Arc<PendingTransaction<T>>>,
    /// _All_ transactions sorted by priority
    all: BTreeSet<PendingTransactionRef<T>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PendingTransactionRef<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`](crate::traits::PoolTransaction::size).
    size_of: SizeTracker,
}

// === impl PendingPool ===

impl<T: TransactionOrdering> PendingPool<T> {
    /// Create a new pool instance.
    pub(crate) fn new(ordering: T) -> Self {
        Self {
            ordering,
            submission_id: 0,
            by_id: Default::default(),
            all: Default::default(),
            independent_transactions: Default::default(),
            size_of: Default::default(),
        }
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
        }
    }

    /// Returns the ancestor the given transaction, the transaction with `nonce - 1`.
    ///
    /// Note: for a transaction with nonce higher than the current on chain nonce this will always
    /// return an ancestor since all transaction in this pool are gapless.
    fn ancestor(&self, id: &TransactionId) -> Option<&Arc<PendingTransaction<T>>> {
        self.by_id.get(&id.unchecked_ancestor()?)
    }

    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// if the transaction is already included
    pub(crate) fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        assert!(
            !self.by_id.contains_key(tx.id()),
            "transaction already included {:?}",
            self.by_id.contains_key(tx.id())
        );

        let tx_id = *tx.id();
        let submission_id = self.next_id();

        let priority = self.ordering.priority(&tx.transaction);

        // keep track of size
        self.size_of += tx.size();

        let transaction = PendingTransactionRef { submission_id, transaction: tx, priority };

        // If there's __no__ ancestor in the pool, then this transaction is independent, this is
        // guaranteed because this pool is gapless.
        if self.ancestor(&tx_id).is_none() {
            self.independent_transactions.insert(transaction.clone());
        }
        self.all.insert(transaction.clone());

        let transaction = Arc::new(PendingTransaction { transaction });

        self.by_id.insert(tx_id, transaction);
    }

    /// Removes a _mined_ transaction from the pool.
    ///
    /// If the transactions has a descendant transaction it will advance it to the best queue.
    pub(crate) fn prune_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // mark the next as independent if it exists
        if let Some(unlocked) = self.by_id.get(&id.descendant()) {
            self.independent_transactions.insert(unlocked.transaction.clone());
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
        // keep track of size
        self.size_of -= tx.transaction.transaction.size();
        self.all.remove(&tx.transaction);
        self.independent_transactions.remove(&tx.transaction);
        Some(tx.transaction.transaction.clone())
    }

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// Removes the worst transaction from this pool.
    pub(crate) fn pop_worst(&mut self) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let worst = self.all.iter().next_back().map(|tx| *tx.transaction.id())?;
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
    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// A transaction that is ready to be included in a block.
pub(crate) struct PendingTransaction<T: TransactionOrdering> {
    /// Reference to the actual transaction.
    pub(crate) transaction: PendingTransactionRef<T>,
}

impl<T: TransactionOrdering> Clone for PendingTransaction<T> {
    fn clone(&self) -> Self {
        Self { transaction: self.transaction.clone() }
    }
}

/// A transaction that is ready to be included in a block.
pub(crate) struct PendingTransactionRef<T: TransactionOrdering> {
    /// Identifier that tags when transaction was submitted in the pool.
    pub(crate) submission_id: u64,
    /// Actual transaction.
    pub(crate) transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// The priority value assigned by the used `Ordering` function.
    pub(crate) priority: T::Priority,
}

impl<T: TransactionOrdering> PendingTransactionRef<T> {
    /// The next transaction of the sender: `nonce + 1`
    pub(crate) fn unlocks(&self) -> TransactionId {
        self.transaction.transaction_id.descendant()
    }
}

impl<T: TransactionOrdering> Clone for PendingTransactionRef<T> {
    fn clone(&self) -> Self {
        Self {
            submission_id: self.submission_id,
            transaction: Arc::clone(&self.transaction),
            priority: self.priority.clone(),
        }
    }
}

impl<T: TransactionOrdering> Eq for PendingTransactionRef<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for PendingTransactionRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for PendingTransactionRef<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for PendingTransactionRef<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `submission_id`. This ensures that transactions with same priority are not
        // equal, so they're not replaced in the set
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}
