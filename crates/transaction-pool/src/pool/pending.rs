use crate::{
    identifier::TransactionId,
    pool::{best::BestTransactions, size::SizeTracker},
    Priority, SubPoolLimit, TransactionOrdering, ValidPoolTransaction,
};

use crate::pool::best::BestTransactionsWithBasefee;
use reth_primitives::Address;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
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

    /// Updates the pool with the new blob fee. Removes
    /// from the subpool all transactions and their dependents that no longer satisfy the given
    /// base fee (`tx.max_blob_fee < blob_fee`).
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
                if self.ancestor(&id).is_none() {
                    self.independent_transactions.insert(tx.clone());
                }
                self.all.insert(tx.clone());
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

    /// Truncates the pool to the given limit.
    ///
    /// Removes transactions from the pending pool
    /// The algorithm tries to reduce transaction counts by an approximately
    /// equal number for all for accounts with many pending transactions.
    pub(crate) fn truncate_pool(
        &mut self,
        limit: SubPoolLimit,
        max_account_slots: usize,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();
        let mut spammers_map = self.get_spammers(max_account_slots);

        let mut spammers =
            spammers_map.iter().map(|(addr, txs)| (*addr, txs.len())).collect::<Vec<_>>();

        // sort by number of txs, in ascending order
        spammers.sort_by(|(_, txs1_len), (_, txs2_len)| txs1_len.cmp(txs2_len));

        for (_, txs) in spammers_map.iter_mut() {
            // sort txs by nonce so we can pop the highest nonce element if necessary
            txs.sort_by(|tx1, tx2| tx1.transaction.nonce().cmp(&tx2.transaction.nonce()));
        }

        // penalize spammers first by equalizing txs count first
        let mut offenders = Vec::new();
        let mut spammer_index = 0;

        // The point of this loop is to iterate through the list of spammers, sorted by tx count,
        // and find the sender where:
        //
        // If all senders with more transactions than the current sender were set to the same
        // number of transactions as the current sender, the pool would be below the limit.
        //
        // If many transactions have around the same number of transactions, not many transactions
        // will be removed.
        while self.len() > limit.max_txs && !spammers.is_empty() {
            // SAFETY: We know this will return Some due to `!spammers.is_empty()` above
            let offender_pair = spammers.pop().unwrap();
            offenders.push(offender_pair);

            println!("offender_pair: {:?}", offender_pair);
            if offenders.len() > 1 {
                let threshold = offender_pair.1; // current spammer txs count

                println!("threshold: {}", threshold);
                // SAFETY: The above condition means that `offenders.len() > 1`, the len will be 2
                // or greater, so offenders[offenders.len() - 2] will not panic.
                //
                // We also know that the inside loop decrements that element's tx_count (`.1`)
                // until is is less than or equal to the `threshold`, so this loop should always
                // terminate.
                while self.len() > limit.max_txs && offenders[offenders.len() - 2].1 > threshold {
                    for current_offender in offenders.iter_mut() {
                        // txs are already sorted by nonce, this should pop the highest nonce
                        // SAFETY: all addresses from offenders are in spammers_map
                        let offender_txs =
                            spammers_map.get_mut(&current_offender.0).unwrap().split_off(threshold);
                        current_offender.1 -= offender_txs.len();

                        // remove the txs exeeding the threshold
                        for tx in offender_txs {
                            if let Some(tx) = self.remove_transaction(tx.transaction.id()) {
                                removed.push(tx);
                            }
                        }
                    }
                }
            }
            spammer_index += 1;
        }

        // If still above threshold, reduce to limit till each offenders tx count is below
        // max_account_slots
        while self.len() > limit.max_txs && !offenders.is_empty() {
            let offender = offenders.pop().unwrap();

            // we do not need to decrement the offender tx_count because we're popping it and these
            // should be removed
            //
            // SAFETY: all addresses from offenders are in spammers_map
            let offender_txs =
                spammers_map.get_mut(&offender.0).unwrap().split_off(max_account_slots);

            for tx in offender_txs {
                if let Some(tx) = self.remove_transaction(tx.transaction.id()) {
                    removed.push(tx);
                }
            }
        }

        // penalize non-local txs if limit is still exceeded
        if self.size() > limit.max_size || self.len() > limit.max_txs {
            for tx in self.all.clone().iter() {
                if tx.transaction.is_local() {
                    continue
                }
                while self.size() > limit.max_size || self.len() > limit.max_txs {
                    if let Some(tx) = self.pop_worst() {
                        removed.push(tx);
                    }
                }
            }
        }

        // penalize local txs at last
        while self.size() > limit.max_size || self.len() > limit.max_txs {
            if let Some(tx) = self.pop_worst() {
                removed.push(tx);
            }
        }

        removed
    }

    /// Returns account address and their txs_count that are _not local_ and whose transaction
    /// count > `max_account_slots` limit.
    ///
    /// This returns a map from address to pending transactions for addresses with over
    /// `max_account_slots` transactions.
    pub(crate) fn get_spammers(
        &self,
        max_account_slots: usize,
    ) -> HashMap<Address, Vec<PendingTransaction<T>>> {
        let mut spammers: HashMap<Address, Vec<PendingTransaction<T>>> = HashMap::new();
        for tx in self.all.iter() {
            if tx.transaction.is_local() {
                continue
            }

            // push the tx or populate with the current tx
            let sender = tx.transaction.sender();
            spammers
                .entry(sender)
                .and_modify(|txs| txs.push(tx.clone()))
                .or_insert(vec![tx.clone()]);
        }

        // filter out accounts that are below the threshold
        spammers.retain(|_, txs| txs.len() > max_account_slots);

        spammers
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

    /// Asserts that the bijection between `by_id` and `all` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(self.by_id.len(), self.all.len(), "by_id.len() != all.len()");
        assert!(
            self.independent_transactions.len() <= self.all.len(),
            "independent.len() > all.len()"
        );
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
    use std::collections::HashSet;

    use reth_primitives::address;

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

    #[test]
    fn truncate_by_sender() {
        // this test ensures that we evict from the pending pool by sender
        // TODO: Ensure local transactions are not evicted
        let mut f = MockTransactionFactory::default();
        let mut pool = PendingPool::new(MockOrdering::default());

        let a = address!("000000000000000000000000000000000000000a");
        let b = address!("000000000000000000000000000000000000000b");
        let c = address!("000000000000000000000000000000000000000c");
        let d = address!("000000000000000000000000000000000000000d");

        // TODO: make creating these mock tx chains easier
        // create a chain of transactions by sender A, B, C
        let a1 = MockTransaction::eip1559().with_sender(a);
        let a2 = a1.clone().with_nonce(1);
        let a3 = a1.clone().with_nonce(2);
        let a4 = a1.clone().with_nonce(3);

        let b1 = MockTransaction::eip1559().with_sender(b);
        let b2 = b1.clone().with_nonce(1);
        let b3 = b1.clone().with_nonce(2);

        // C has the same number of txs as B
        let c1 = MockTransaction::eip1559().with_sender(c);
        let c2 = c1.clone().with_nonce(1);
        let c3 = c1.clone().with_nonce(2);

        // D is not a spammer
        let d1 = MockTransaction::eip1559().with_sender(d);

        // just construct a list of all txs to add
        let expected_pending = vec![a1.clone(), b1.clone(), c1.clone(), d1.clone()];
        let expected_removed = vec![
            a2.clone(),
            a3.clone(),
            a4.clone(),
            b2.clone(),
            b3.clone(),
            c2.clone(),
            c3.clone(),
        ];
        let all_txs = vec![a1, a2, a3, a4, b1, b2, b3, c1, c2, c3, d1];

        // add all the transactions to the pool
        for tx in all_txs {
            println!("adding tx {:?}, has id: {:?}", tx, f.tx_id(&tx));
            pool.add_transaction(f.validated_arc(tx), 0);
        }

        // let's set the max_account_slots to 2, and the max total txs to 3, we should end up with
        // only the first transactions
        let max_account_slots = 2;
        let pool_limit = SubPoolLimit {
            max_txs: 3,
            // TODO: size is going to make this complicated i think....
            max_size: usize::MAX,
        };

        // find the spammers - this should be A, B, C, but not D
        let spammers = pool.get_spammers(max_account_slots);
        let spammers =
            spammers.into_iter().map(|(addr, txs)| (addr, txs.len())).collect::<HashSet<_>>();
        let expected_spammers = [(a, 4), (b, 3), (c, 3usize)];
        let expected_spammers = expected_spammers.into_iter().collect::<HashSet<_>>();
        assert_eq!(spammers, expected_spammers);

        // truncate the pool
        let removed = pool.truncate_pool(pool_limit, max_account_slots);
        assert_eq!(removed.len(), expected_removed.len());

        // get the inner txs from the removed txs
        let removed = removed.into_iter().map(|tx| tx.transaction.clone()).collect::<Vec<_>>();
        assert_eq!(removed, expected_removed);

        // get the pending pool
        let pending = pool.all().collect::<Vec<_>>();
        assert_eq!(pending.len(), expected_pending.len());

        // get the inner txs from the pending txs
        let pending = pending.into_iter().map(|tx| tx.transaction.clone()).collect::<Vec<_>>();
        assert_eq!(pending, expected_pending);
    }
}
