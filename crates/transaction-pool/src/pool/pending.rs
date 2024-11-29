use crate::{
    identifier::{SenderId, TransactionId},
    pool::{
        best::{BestTransactions, BestTransactionsWithFees},
        size::SizeTracker,
    },
    Priority, SubPoolLimit, TransactionOrdering, ValidPoolTransaction,
};
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeMap, HashMap},
    ops::Bound::Unbounded,
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
#[derive(Debug, Clone)]
pub struct PendingPool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: T,
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions were submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, PendingTransaction<T>>,
    /// The highest nonce transactions for each sender - like the `independent` set, but the
    /// highest instead of lowest nonce.
    highest_nonces: HashMap<SenderId, PendingTransaction<T>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    independent_transactions: HashMap<SenderId, PendingTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`](crate::traits::PoolTransaction::size).
    size_of: SizeTracker,
    /// Used to broadcast new transactions that have been added to the `PendingPool` to existing
    /// `static_files` of this pool.
    new_transaction_notifier: broadcast::Sender<PendingTransaction<T>>,
}

// === impl PendingPool ===

impl<T: TransactionOrdering> PendingPool<T> {
    /// Create a new pool instance.
    pub fn new(ordering: T) -> Self {
        let (new_transaction_notifier, _) = broadcast::channel(200);
        Self {
            ordering,
            submission_id: 0,
            by_id: Default::default(),
            independent_transactions: Default::default(),
            highest_nonces: Default::default(),
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
        self.highest_nonces.clear();
        self.size_of.reset();
        std::mem::take(&mut self.by_id)
    }

    /// Returns an iterator over all transactions that are _currently_ ready.
    ///
    /// 1. The iterator _always_ returns transactions in order: it never returns a transaction with
    ///    an unsatisfied dependency and only returns them if dependency transaction were yielded
    ///    previously. In other words: the nonces of transactions with the same sender will _always_
    ///    increase by exactly 1.
    ///
    /// The order of transactions which satisfy (1.) is determined by their computed priority: a
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
            independent: self.independent_transactions.values().cloned().collect(),
            invalid: Default::default(),
            new_transaction_receiver: Some(self.new_transaction_notifier.subscribe()),
            skip_blobs: false,
        }
    }

    /// Same as `best` but only returns transactions that satisfy the given basefee and blobfee.
    pub(crate) fn best_with_basefee_and_blobfee(
        &self,
        base_fee: u64,
        base_fee_per_blob_gas: u64,
    ) -> BestTransactionsWithFees<T> {
        BestTransactionsWithFees { best: self.best(), base_fee, base_fee_per_blob_gas }
    }

    /// Same as `best` but also includes the given unlocked transactions.
    ///
    /// This mimics the [`Self::add_transaction`] method, but does not insert the transactions into
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

    /// Updates the pool with the new blob fee. Removes
    /// from the subpool all transactions and their dependents that no longer satisfy the given
    /// blob fee (`tx.max_blob_fee < blob_fee`).
    ///
    /// Note: the transactions are not returned in a particular order.
    ///
    /// # Returns
    ///
    /// Removed transactions that no longer satisfy the blob fee.
    pub(crate) fn update_blob_fee(
        &mut self,
        blob_fee: u128,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        // Create a collection for removed transactions.
        let mut removed = Vec::new();

        // Drain and iterate over all transactions.
        let mut transactions_iter = self.clear_transactions().into_iter().peekable();
        while let Some((id, tx)) = transactions_iter.next() {
            if tx.transaction.max_fee_per_blob_gas() < Some(blob_fee) {
                // Add this tx to the removed collection since it no longer satisfies the blob fee
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
                self.size_of += tx.transaction.size();
                self.update_independents_and_highest_nonces(&tx);
                self.by_id.insert(id, tx);
            }
        }

        removed
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
                self.update_independents_and_highest_nonces(&tx);
                self.by_id.insert(id, tx);
            }
        }

        removed
    }

    /// Updates the independent transaction and highest nonces set, assuming the given transaction
    /// is being _added_ to the pool.
    fn update_independents_and_highest_nonces(&mut self, tx: &PendingTransaction<T>) {
        match self.highest_nonces.entry(tx.transaction.sender_id()) {
            Entry::Occupied(mut entry) => {
                if entry.get().transaction.nonce() < tx.transaction.nonce() {
                    *entry.get_mut() = tx.clone();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(tx.clone());
            }
        }
        match self.independent_transactions.entry(tx.transaction.sender_id()) {
            Entry::Occupied(mut entry) => {
                if entry.get().transaction.nonce() > tx.transaction.nonce() {
                    *entry.get_mut() = tx.clone();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(tx.clone());
            }
        }
    }

    /// Returns the ancestor the given transaction, the transaction with `nonce - 1`.
    ///
    /// Note: for a transaction with nonce higher than the current on chain nonce this will always
    /// return an ancestor since all transaction in this pool are gapless.
    fn ancestor(&self, id: &TransactionId) -> Option<&PendingTransaction<T>> {
        self.get(&id.unchecked_ancestor()?)
    }

    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// if the transaction is already included
    pub fn add_transaction(
        &mut self,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
        base_fee: u64,
    ) {
        assert!(
            !self.contains(tx.id()),
            "transaction already included {:?}",
            self.get(tx.id()).unwrap().transaction
        );

        // keep track of size
        self.size_of += tx.size();

        let tx_id = *tx.id();

        let submission_id = self.next_id();
        let priority = self.ordering.priority(&tx.transaction, base_fee);
        let tx = PendingTransaction { submission_id, transaction: tx, priority };

        self.update_independents_and_highest_nonces(&tx);

        // send the new transaction to any existing pendingpool static file iterators
        if self.new_transaction_notifier.receiver_count() > 0 {
            let _ = self.new_transaction_notifier.send(tx.clone());
        }

        self.by_id.insert(tx_id, tx);
    }

    /// Removes the transaction from the pool.
    ///
    /// Note: If the transaction has a descendant transaction
    /// it will advance it to the best queue.
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        if let Some(lowest) = self.independent_transactions.get(&id.sender) {
            if lowest.transaction.nonce() == id.nonce {
                self.independent_transactions.remove(&id.sender);
                // mark the next as independent if it exists
                if let Some(unlocked) = self.get(&id.descendant()) {
                    self.independent_transactions.insert(id.sender, unlocked.clone());
                }
            }
        }

        let tx = self.by_id.remove(id)?;
        self.size_of -= tx.transaction.size();

        if let Some(highest) = self.highest_nonces.get(&id.sender) {
            if highest.transaction.nonce() == id.nonce {
                self.highest_nonces.remove(&id.sender);
            }
            if let Some(ancestor) = self.ancestor(id) {
                self.highest_nonces.insert(id.sender, ancestor.clone());
            }
        }
        Some(tx.transaction)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// Traverses the pool, starting at the highest nonce set, removing the transactions which
    /// would put the pool under the specified limits.
    ///
    /// This attempts to remove transactions by roughly the same amount for each sender. This is
    /// done by removing the highest-nonce transactions for each sender.
    ///
    /// If the `remove_locals` flag is unset, transactions will be removed per-sender until a
    /// local transaction is the highest nonce transaction for that sender. If all senders have a
    /// local highest-nonce transaction, the pool will not be truncated further.
    ///
    /// Otherwise, if the `remove_locals` flag is set, transactions will be removed per-sender
    /// until the pool is under the given limits.
    ///
    /// Any removed transactions will be added to the `end_removed` vector.
    pub fn remove_to_limit(
        &mut self,
        limit: &SubPoolLimit,
        remove_locals: bool,
        end_removed: &mut Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        // This serves as a termination condition for the loop - it represents the number of
        // _valid_ unique senders that might have descendants in the pool.
        //
        // If `remove_locals` is false, a value of zero means that there are no non-local txs in the
        // pool that can be removed.
        //
        // If `remove_locals` is true, a value of zero means that there are no txs in the pool that
        // can be removed.
        let mut non_local_senders = self.highest_nonces.len();

        // keep track of unique senders from previous iterations, to understand how many unique
        // senders were removed in the last iteration
        let mut unique_senders = self.highest_nonces.len();

        // keep track of transactions to remove and how many have been removed so far
        let original_length = self.len();
        let mut removed = Vec::new();
        let mut total_removed = 0;

        // track total `size` of transactions to remove
        let original_size = self.size();
        let mut total_size = 0;

        loop {
            // check how many unique senders were removed last iteration
            let unique_removed = unique_senders - self.highest_nonces.len();

            // the new number of unique senders
            unique_senders = self.highest_nonces.len();
            non_local_senders -= unique_removed;

            // we can reuse the temp array
            removed.clear();

            // we prefer removing transactions with lower ordering
            let mut worst_transactions = self.highest_nonces.values().collect::<Vec<_>>();
            worst_transactions.sort();

            // loop through the highest nonces set, removing transactions until we reach the limit
            for tx in worst_transactions {
                // return early if the pool is under limits
                if !limit.is_exceeded(original_length - total_removed, original_size - total_size) ||
                    non_local_senders == 0
                {
                    // need to remove remaining transactions before exiting
                    for id in &removed {
                        if let Some(tx) = self.remove_transaction(id) {
                            end_removed.push(tx);
                        }
                    }

                    return
                }

                if !remove_locals && tx.transaction.is_local() {
                    non_local_senders -= 1;
                    continue
                }

                total_size += tx.transaction.size();
                total_removed += 1;
                removed.push(*tx.transaction.id());
            }

            // remove the transactions from this iteration
            for id in &removed {
                if let Some(tx) = self.remove_transaction(id) {
                    end_removed.push(tx);
                }
            }

            // return if either the pool is under limits or there are no more _eligible_
            // transactions to remove
            if !self.exceeds(limit) || non_local_senders == 0 {
                return
            }
        }
    }

    /// Truncates the pool to the given [`SubPoolLimit`], removing transactions until the subpool
    /// limits are met.
    ///
    /// This attempts to remove transactions by roughly the same amount for each sender. For more
    /// information on this exact process see docs for
    /// [`remove_to_limit`](PendingPool::remove_to_limit).
    ///
    /// This first truncates all of the non-local transactions in the pool. If the subpool is still
    /// not under the limit, this truncates the entire pool, including non-local transactions. The
    /// removed transactions are returned.
    pub fn truncate_pool(
        &mut self,
        limit: SubPoolLimit,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();
        // return early if the pool is already under the limits
        if !self.exceeds(&limit) {
            return removed
        }

        // first truncate only non-local transactions, returning if the pool end up under the limit
        self.remove_to_limit(&limit, false, &mut removed);
        if !self.exceeds(&limit) {
            return removed
        }

        // now repeat for local transactions, since local transactions must be removed now for the
        // pool to be under the limit
        self.remove_to_limit(&limit, true, &mut removed);

        removed
    }

    /// Returns true if the pool exceeds the given limit
    #[inline]
    pub(crate) fn exceeds(&self, limit: &SubPoolLimit) -> bool {
        limit.is_exceeded(self.len(), self.size())
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
    pub(crate) fn contains(&self, id: &TransactionId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Get transactions by sender
    pub(crate) fn get_txs_by_sender(&self, sender: SenderId) -> Vec<TransactionId> {
        self.by_id
            .range((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
            .map(|(tx_id, _)| *tx_id)
            .collect()
    }

    /// Retrieves a transaction with the given ID from the pool, if it exists.
    fn get(&self, id: &TransactionId) -> Option<&PendingTransaction<T>> {
        self.by_id.get(id)
    }

    /// Returns a reference to the independent transactions in the pool
    #[cfg(test)]
    pub(crate) const fn independent(&self) -> &HashMap<SenderId, PendingTransaction<T>> {
        &self.independent_transactions
    }

    /// Asserts that the bijection between `by_id` and `all` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert!(
            self.independent_transactions.len() <= self.by_id.len(),
            "independent.len() > all.len()"
        );
        assert!(
            self.highest_nonces.len() <= self.by_id.len(),
            "independent_descendants.len() > all.len()"
        );
        assert_eq!(
            self.highest_nonces.len(),
            self.independent_transactions.len(),
            "independent.len() = independent_descendants.len()"
        );
    }
}

/// A transaction that is ready to be included in a block.
#[derive(Debug)]
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
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory, MockTransactionSet},
        PoolTransaction,
    };
    use alloy_primitives::address;
    use reth_primitives::TxType;
    use std::collections::HashSet;

    #[test]
    fn test_enforce_basefee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx.clone(), 0);

        assert!(pool.contains(tx.id()));
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

        assert!(pool.contains(root_tx.id()));
        assert!(pool.contains(descendant_tx.id()));
        assert_eq!(pool.len(), 2);

        assert_eq!(pool.independent_transactions.len(), 1);
        assert_eq!(pool.highest_nonces.len(), 1);

        let removed = pool.update_base_fee(0);
        assert!(removed.is_empty());

        // two dependent tx in the pool with decreasing fee

        {
            let mut pool2 = pool.clone();
            let removed = pool2.update_base_fee((descendant_tx.max_fee_per_gas() + 1) as u64);
            assert_eq!(removed.len(), 1);
            assert_eq!(pool2.len(), 1);
            // descendant got popped
            assert!(pool2.contains(root_tx.id()));
            assert!(!pool2.contains(descendant_tx.id()));
        }

        // remove root transaction via fee
        let removed = pool.update_base_fee((root_tx.max_fee_per_gas() + 1) as u64);
        assert_eq!(removed.len(), 2);
        assert!(pool.is_empty());
        pool.assert_invariants();
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
        assert_eq!(
            pool.highest_nonces.values().min().map(|tx| *tx.transaction.hash()),
            Some(*t.hash())
        );

        // truncate pool with max size = 1, ensure it's the same transaction
        let removed = pool.truncate_pool(SubPoolLimit { max_txs: 1, max_size: usize::MAX });
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), t.hash());
    }

    #[test]
    fn correct_independent_descendants() {
        // this test ensures that we set the right highest nonces set for each sender
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        let a_sender = address!("000000000000000000000000000000000000000a");
        let b_sender = address!("000000000000000000000000000000000000000b");
        let c_sender = address!("000000000000000000000000000000000000000c");
        let d_sender = address!("000000000000000000000000000000000000000d");

        // create a chain of transactions by sender A, B, C
        let mut tx_set =
            MockTransactionSet::dependent(a_sender, 0, 4, reth_primitives::TxType::Eip1559);
        let a = tx_set.clone().into_vec();

        let b = MockTransactionSet::dependent(b_sender, 0, 3, reth_primitives::TxType::Eip1559)
            .into_vec();
        tx_set.extend(b.clone());

        // C has the same number of txs as B
        let c = MockTransactionSet::dependent(c_sender, 0, 3, reth_primitives::TxType::Eip1559)
            .into_vec();
        tx_set.extend(c.clone());

        let d = MockTransactionSet::dependent(d_sender, 0, 1, reth_primitives::TxType::Eip1559)
            .into_vec();
        tx_set.extend(d.clone());

        // add all the transactions to the pool
        let all_txs = tx_set.into_vec();
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx), 0);
        }

        pool.assert_invariants();

        // the independent set is the roots of each of these tx chains, these are the highest
        // nonces for each sender
        let expected_highest_nonces = vec![d[0].clone(), c[2].clone(), b[2].clone(), a[3].clone()]
            .iter()
            .map(|tx| (tx.sender(), tx.nonce()))
            .collect::<HashSet<_>>();
        let actual_highest_nonces = pool
            .highest_nonces
            .values()
            .map(|tx| (tx.transaction.sender(), tx.transaction.nonce()))
            .collect::<HashSet<_>>();
        assert_eq!(expected_highest_nonces, actual_highest_nonces);
        pool.assert_invariants();
    }

    #[test]
    fn truncate_by_sender() {
        // This test ensures that transactions are removed from the pending pool by sender.
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        // Addresses for simulated senders A, B, C, and D.
        let a = address!("000000000000000000000000000000000000000a");
        let b = address!("000000000000000000000000000000000000000b");
        let c = address!("000000000000000000000000000000000000000c");
        let d = address!("000000000000000000000000000000000000000d");

        // Create transaction chains for senders A, B, C, and D.
        let a_txs = MockTransactionSet::sequential_transactions_by_sender(a, 4, TxType::Eip1559);
        let b_txs = MockTransactionSet::sequential_transactions_by_sender(b, 3, TxType::Eip1559);
        let c_txs = MockTransactionSet::sequential_transactions_by_sender(c, 3, TxType::Eip1559);
        let d_txs = MockTransactionSet::sequential_transactions_by_sender(d, 1, TxType::Eip1559);

        // Set up expected pending transactions.
        let expected_pending = vec![
            a_txs.transactions[0].clone(),
            b_txs.transactions[0].clone(),
            c_txs.transactions[0].clone(),
            a_txs.transactions[1].clone(),
        ]
        .into_iter()
        .map(|tx| (tx.sender(), tx.nonce()))
        .collect::<HashSet<_>>();

        // Set up expected removed transactions.
        let expected_removed = vec![
            d_txs.transactions[0].clone(),
            c_txs.transactions[2].clone(),
            b_txs.transactions[2].clone(),
            a_txs.transactions[3].clone(),
            c_txs.transactions[1].clone(),
            b_txs.transactions[1].clone(),
            a_txs.transactions[2].clone(),
        ]
        .into_iter()
        .map(|tx| (tx.sender(), tx.nonce()))
        .collect::<HashSet<_>>();

        // Consolidate all transactions into a single vector.
        let all_txs =
            [a_txs.into_vec(), b_txs.into_vec(), c_txs.into_vec(), d_txs.into_vec()].concat();

        // Add all the transactions to the pool.
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx), 0);
        }

        // Sanity check, ensuring everything is consistent.
        pool.assert_invariants();

        // Define the maximum total transactions to be 4, removing transactions for each sender.
        // Expected order of removal:
        // * d1, c3, b3, a4
        // * c2, b2, a3
        //
        // Remaining transactions:
        // * a1, a2
        // * b1
        // * c1
        let pool_limit = SubPoolLimit { max_txs: 4, max_size: usize::MAX };

        // Truncate the pool based on the defined limit.
        let removed = pool.truncate_pool(pool_limit);
        pool.assert_invariants();
        assert_eq!(removed.len(), expected_removed.len());

        // Get the set of removed transactions and compare with the expected set.
        let removed =
            removed.into_iter().map(|tx| (tx.sender(), tx.nonce())).collect::<HashSet<_>>();
        assert_eq!(removed, expected_removed);

        // Retrieve the current pending transactions after truncation.
        let pending = pool.all().collect::<Vec<_>>();
        assert_eq!(pending.len(), expected_pending.len());

        // Get the set of pending transactions and compare with the expected set.
        let pending =
            pending.into_iter().map(|tx| (tx.sender(), tx.nonce())).collect::<HashSet<_>>();
        assert_eq!(pending, expected_pending);
    }

    // <https://github.com/paradigmxyz/reth/issues/12340>
    #[test]
    fn test_eligible_updates_promoted() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_senders = 10;

        let first_txs: Vec<_> = (0..num_senders) //
            .map(|_| MockTransaction::eip1559())
            .collect();
        let second_txs: Vec<_> =
            first_txs.iter().map(|tx| tx.clone().rng_hash().inc_nonce()).collect();

        for tx in first_txs {
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best();

        for _ in 0..num_senders {
            if let Some(tx) = best.next() {
                assert_eq!(tx.nonce(), 0);
            } else {
                panic!("cannot read one of first_txs");
            }
        }

        for tx in second_txs {
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        for _ in 0..num_senders {
            if let Some(tx) = best.next() {
                assert_eq!(tx.nonce(), 1);
            } else {
                panic!("cannot read one of second_txs");
            }
        }
    }

    #[test]
    fn test_empty_pool_behavior() {
        let mut pool = PendingPool::<MockOrdering>::new(MockOrdering::default());

        // Ensure the pool is empty
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.size(), 0);

        // Verify that attempting to truncate an empty pool does not panic and returns an empty vec
        let removed = pool.truncate_pool(SubPoolLimit { max_txs: 10, max_size: 1000 });
        assert!(removed.is_empty());

        // Verify that retrieving transactions from an empty pool yields nothing
        let all_txs: Vec<_> = pool.all().collect();
        assert!(all_txs.is_empty());
    }

    #[test]
    fn test_add_remove_transaction() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        // Add a transaction and check if it's in the pool
        let tx = f.validated_arc(MockTransaction::eip1559());
        pool.add_transaction(tx.clone(), 0);
        assert!(pool.contains(tx.id()));
        assert_eq!(pool.len(), 1);

        // Remove the transaction and ensure it's no longer in the pool
        let removed_tx = pool.remove_transaction(tx.id()).unwrap();
        assert_eq!(removed_tx.id(), tx.id());
        assert!(!pool.contains(tx.id()));
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_reorder_on_basefee_update() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        // Add two transactions with different fees
        let tx1 = f.validated_arc(MockTransaction::eip1559().inc_price());
        let tx2 = f.validated_arc(MockTransaction::eip1559().inc_price_by(20));
        pool.add_transaction(tx1.clone(), 0);
        pool.add_transaction(tx2.clone(), 0);

        // Ensure the transactions are in the correct order
        let mut best = pool.best();
        assert_eq!(best.next().unwrap().hash(), tx2.hash());
        assert_eq!(best.next().unwrap().hash(), tx1.hash());

        // Update the base fee to a value higher than tx1's fee, causing it to be removed
        let removed = pool.update_base_fee((tx1.max_fee_per_gas() + 1) as u64);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), tx1.hash());

        // Verify that only tx2 remains in the pool
        assert_eq!(pool.len(), 1);
        assert!(pool.contains(tx2.id()));
        assert!(!pool.contains(tx1.id()));
    }

    #[test]
    #[should_panic(expected = "transaction already included")]
    fn test_handle_duplicates() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        // Add the same transaction twice and ensure it only appears once
        let tx = f.validated_arc(MockTransaction::eip1559());
        pool.add_transaction(tx.clone(), 0);
        assert!(pool.contains(tx.id()));
        assert_eq!(pool.len(), 1);

        // Attempt to add the same transaction again, which should be ignored
        pool.add_transaction(tx, 0);
    }

    #[test]
    fn test_update_blob_fee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        // Add transactions with varying blob fees
        let tx1 = f.validated_arc(MockTransaction::eip4844().set_blob_fee(50).clone());
        let tx2 = f.validated_arc(MockTransaction::eip4844().set_blob_fee(150).clone());
        pool.add_transaction(tx1.clone(), 0);
        pool.add_transaction(tx2.clone(), 0);

        // Update the blob fee to a value that causes tx1 to be removed
        let removed = pool.update_blob_fee(100);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), tx1.hash());

        // Verify that only tx2 remains in the pool
        assert!(pool.contains(tx2.id()));
        assert!(!pool.contains(tx1.id()));
    }
}
