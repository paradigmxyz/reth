use crate::{
    error::PoolResult, identifier::TransactionId, pool::queued::QueuedPoolTransaction,
    traits::BestTransactions, validate::ValidPoolTransaction, TransactionOrdering,
};
use parking_lot::RwLock;
use reth_primitives::{TxHash, U256};
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};
use tracing::debug;

/// Type alias for replaced transactions
pub(crate) type ReplacedTransactions<T> =
    (Vec<Arc<ValidPoolTransaction<<T as TransactionOrdering>::Transaction>>>, Vec<TxHash>);

/// A pool of validated transactions that are ready on the current state and are waiting to be
/// included in a block.
///
/// Each transaction in this pool is valid on its own, i.e. they are not dependent on transaction
/// that must be executed first. Each of these transaction can be executed independently on the
/// current state
pub(crate) struct PendingTransactions<T: TransactionOrdering> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// How to order transactions.
    ordering: Arc<T>,
    /// Base fee of the next block.
    pending_base_fee: U256,
    /// Dependencies that are provided by `PendingTransaction`s
    provided_dependencies: HashMap<TransactionId, TxHash>,
    /// Pending transactions that are currently on hold until the `baseFee` of the pending block
    /// changes in favor of the parked transactions: the `pendingBlock.baseFee` must decrease
    /// before they can be moved to the ready pool and are ready to be executed.
    parked: ParkedTransactions<T>,
    /// All Transactions that are currently ready.
    ///
    /// Meaning, there are no nonce gaps in these transactions and all of them satisfy the
    /// `baseFee` condition: transaction `maxFeePerGas >= pendingBlock.baseFee`
    ready_transactions: Arc<RwLock<HashMap<TxHash, PendingTransaction<T>>>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PoolTransactionRef<T>>,
}

// === impl PendingTransactions ===

impl<T: TransactionOrdering> PendingTransactions<T> {
    /// Create a new pool instance
    pub(crate) fn new(ordering: Arc<T>) -> Self {
        Self {
            id: 0,
            provided_dependencies: Default::default(),
            parked: Default::default(),
            ready_transactions: Arc::new(Default::default()),
            ordering,
            independent_transactions: Default::default(),
            pending_base_fee: Default::default(),
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
    pub(crate) fn get_transactions(&self) -> TransactionsIterator<T> {
        TransactionsIterator {
            all: self.ready_transactions.read().clone(),
            independent: self.independent_transactions.clone(),
            awaiting: Default::default(),
            invalid: Default::default(),
        }
    }

    /// Sets the given base fee and returns the old one.
    pub(crate) fn set_next_base_fee(&mut self, base_fee: U256) -> U256 {
        std::mem::replace(&mut self.pending_base_fee, base_fee)
    }

    /// Returns true if the transaction is part of the queue.
    pub(crate) fn contains(&self, hash: &TxHash) -> bool {
        self.ready_transactions.read().contains_key(hash)
    }

    /// Returns the transaction for the hash if it's in the ready pool but not yet mined
    pub(crate) fn get(&self, hash: &TxHash) -> Option<PendingTransaction<T>> {
        self.ready_transactions.read().get(hash).cloned()
    }

    pub(crate) fn provided_dependencies(&self) -> &HashMap<TransactionId, TxHash> {
        &self.provided_dependencies
    }

    fn next_id(&mut self) -> u64 {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        id
    }

    /// Adds a new transactions to the pending queue.
    ///
    /// Depending on the transaction's feeCap, this will either move it into the ready queue or park
    /// it until a future baseFee unlocks it.
    ///
    /// # Panics
    ///
    /// if the pending transaction is not ready
    /// or the transaction is already included
    pub(crate) fn add_transaction(
        &mut self,
        tx: QueuedPoolTransaction<T>,
    ) -> PoolResult<Vec<Arc<ValidPoolTransaction<T::Transaction>>>> {
        assert!(tx.is_satisfied(), "transaction must be ready",);
        assert!(
            !self.ready_transactions.read().contains_key(tx.transaction.hash()),
            "transaction already included"
        );

        let (replaced_tx, unlocks) = self.replaced_transactions(&tx.transaction)?;

        let submission_id = self.next_id();
        let hash = *tx.transaction.hash();

        let mut independent = true;
        let mut requires_offset = 0;
        let mut ready = self.ready_transactions.write();

        // Add links to transactions that unlock the current one
        for dependency in &tx.transaction.depends_on {
            // Check if the transaction that satisfies the mark is still in the queue.
            if let Some(other) = self.provided_dependencies.get(dependency) {
                let tx = ready.get_mut(other).expect("hash included;");
                tx.unlocks.push(hash);
                // tx still depends on other tx
                independent = false;
            } else {
                requires_offset += 1;
            }
        }

        // update dependencies
        self.provided_dependencies.insert(tx.transaction.transaction_id, hash);

        let priority = self.ordering.priority(&tx.transaction.transaction);

        let transaction =
            PoolTransactionRef { submission_id, transaction: tx.transaction, priority };

        // TODO check basefee requirement

        // add to the independent set
        if independent {
            self.independent_transactions.insert(transaction.clone());
        }

        // insert to ready queue
        ready.insert(hash, PendingTransaction { transaction, unlocks, requires_offset });

        Ok(replaced_tx)
    }

    /// Removes and returns those transactions that got replaced by the `tx`
    fn replaced_transactions(
        &mut self,
        tx: &ValidPoolTransaction<T::Transaction>,
    ) -> PoolResult<ReplacedTransactions<T>> {
        // check if we are replacing transactions
        let remove_hashes: HashSet<_> =
            if let Some(hash) = self.provided_dependencies.get(&tx.transaction_id) {
                HashSet::from([hash])
            } else {
                return Ok((Vec::new(), Vec::new()))
            };

        // early exit if we are not replacing anything.
        if remove_hashes.is_empty() {
            return Ok((Vec::new(), Vec::new()))
        }

        // check if we're replacing the same transaction and if it can be replaced

        let mut unlocked_tx = Vec::new();
        {
            // construct a list of unlocked transactions
            // also check for transactions that shouldn't be replaced because underpriced
            let ready = self.ready_transactions.read();
            for to_remove in remove_hashes.iter().filter_map(|hash| ready.get(hash)) {
                // if we're attempting to replace a transaction that provides the exact same
                // dependencies (addr + nonce) then we check for gas price
                if to_remove.id().eq(&tx.transaction_id) {
                    // check if underpriced
                    // TODO check if underpriced
                    // if tx.pending_transaction.transaction.gas_price() <= to_remove.gas_price() {
                    //     warn!(target: "txpool", "ready replacement transaction underpriced
                    // [{:?}]", tx.hash());     return
                    // Err(PoolError::ReplacementUnderpriced(Box::new(tx.clone())))
                    // } else {
                    //     trace!(target: "txpool", "replacing ready transaction [{:?}] with higher
                    // gas price [{:?}]", to_remove.transaction.transaction.hash(), tx.hash());
                    // }
                }

                unlocked_tx.extend(to_remove.unlocks.iter().cloned())
            }
        }

        let remove_hashes = remove_hashes.into_iter().copied().collect::<Vec<_>>();

        let new_provides = HashSet::from([tx.transaction_id]);
        let removed_tx = self.remove_with_dependencies(remove_hashes, Some(new_provides));

        Ok((removed_tx, unlocked_tx))
    }

    /// Removes the transactions from the ready queue and returns the removed transactions.
    /// This will also remove all transactions that depend on those.
    pub(crate) fn clear_transactions(
        &mut self,
        tx_hashes: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.remove_with_dependencies(tx_hashes.to_vec(), None)
    }

    /// Removes the transactions that was mined.
    ///
    /// This will also remove all transactions that lead to the transaction that provides the
    /// id.
    pub(crate) fn remove_mined(
        &mut self,
        id: TransactionId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed_tx = vec![];

        // the dependencies to remove
        let mut remove = vec![id];

        while let Some(dependency) = remove.pop() {
            let res = self
                .provided_dependencies
                .remove(&dependency)
                .and_then(|hash| self.ready_transactions.write().remove(&hash));

            if let Some(tx) = res {
                let unlocks = tx.unlocks;
                self.independent_transactions.remove(&tx.transaction);
                let tx = tx.transaction.transaction;

                // also remove previous transactions
                {
                    let hash = tx.hash();
                    let mut ready = self.ready_transactions.write();

                    let mut previous_dependency = |dependency| -> Option<Vec<TransactionId>> {
                        let prev_hash = self.provided_dependencies.get(dependency)?;
                        let tx2 = ready.get_mut(prev_hash)?;
                        // remove hash
                        if let Some(idx) = tx2.unlocks.iter().position(|i| i == hash) {
                            tx2.unlocks.swap_remove(idx);
                        }
                        if tx2.unlocks.is_empty() {
                            Some(vec![tx2.transaction.transaction.transaction_id])
                        } else {
                            None
                        }
                    };

                    // find previous transactions
                    for dep in &tx.depends_on {
                        if let Some(mut dependency_to_remove) = previous_dependency(dep) {
                            remove.append(&mut dependency_to_remove);
                        }
                    }
                }

                // add the transactions that just got unlocked to independent set
                for hash in unlocks {
                    if let Some(tx) = self.ready_transactions.write().get_mut(&hash) {
                        tx.requires_offset += 1;
                        if tx.requires_offset == tx.transaction.transaction.depends_on.len() {
                            self.independent_transactions.insert(tx.transaction.clone());
                        }
                    }
                }
                // finally, remove the dependencies that this transaction provides
                let current_dependency = &dependency;
                let removed = self.provided_dependencies.remove(&tx.transaction_id);
                assert_eq!(
                        removed.as_ref(),
                        if current_dependency.eq(&tx.transaction_id) { None } else { Some(tx.hash()) },
                        "The pool contains exactly one transaction providing given tag; the removed transaction
						claims to provide that tag, so it has to be mapped to it's hash; qed"
                    );
                removed_tx.push(tx);
            }
        }

        removed_tx
    }

    /// Removes transactions and those that depend on them and satisfy at least one dependency in
    /// the given filter set.
    pub(crate) fn remove_with_dependencies(
        &mut self,
        mut tx_hashes: Vec<TxHash>,
        dependency_filter: Option<HashSet<TransactionId>>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();
        let mut ready = self.ready_transactions.write();

        while let Some(hash) = tx_hashes.pop() {
            if let Some(mut tx) = ready.remove(&hash) {
                let id = &tx.transaction.transaction.transaction_id;

                // remove the transactions
                let removed_transaction = if dependency_filter
                    .as_ref()
                    .map(|filter| !filter.contains(id))
                    .unwrap_or(true)
                {
                    self.provided_dependencies.remove(id);
                    true
                } else {
                    false
                };

                // remove from unlocks
                for dependency in &tx.transaction.transaction.depends_on {
                    if let Some(hash) = self.provided_dependencies.get(dependency) {
                        if let Some(tx) = ready.get_mut(hash) {
                            if let Some(idx) = tx.unlocks.iter().position(|i| i == hash) {
                                tx.unlocks.swap_remove(idx);
                            }
                        }
                    }
                }

                // remove from the independent set
                self.independent_transactions.remove(&tx.transaction);

                if removed_transaction {
                    // remove all transactions that the current one unlocks
                    tx_hashes.append(&mut tx.unlocks);
                }

                // remove transaction
                removed.push(tx.transaction.transaction);
            }
        }

        removed
    }
}

/// A transaction that is ready to be included in a block.
#[derive(Debug)]
pub(crate) struct PendingTransaction<T: TransactionOrdering> {
    /// Reference to the actual transaction.
    transaction: PoolTransactionRef<T>,
    /// Tracks the transactions that get unlocked by this transaction.
    unlocks: Vec<TxHash>,
    /// Amount of required dependencies that are inherently provided.
    requires_offset: usize,
}

// == impl PendingTransaction ===

impl<T: TransactionOrdering> PendingTransaction<T> {
    /// Returns all ids this transaction satisfies.
    pub(crate) fn id(&self) -> &TransactionId {
        &self.transaction.transaction.transaction_id
    }
}

impl<T: TransactionOrdering> Clone for PendingTransaction<T> {
    fn clone(&self) -> Self {
        Self {
            transaction: self.transaction.clone(),
            unlocks: self.unlocks.clone(),
            requires_offset: self.requires_offset,
        }
    }
}

/// A reference to a transaction in the _pending_ pool
#[derive(Debug)]
pub(crate) struct PoolTransactionRef<T: TransactionOrdering> {
    /// Actual transaction.
    pub(crate) transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// Identifier that tags when transaction was submitted in the pool.
    pub(crate) submission_id: u64,
    /// The priority value assigned by the used `Ordering` function.
    pub(crate) priority: T::Priority,
}

impl<T: TransactionOrdering> Clone for PoolTransactionRef<T> {
    fn clone(&self) -> Self {
        Self {
            transaction: Arc::clone(&self.transaction),
            submission_id: self.submission_id,
            priority: self.priority.clone(),
        }
    }
}

impl<T: TransactionOrdering> Eq for PoolTransactionRef<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for PoolTransactionRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for PoolTransactionRef<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for PoolTransactionRef<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `submission_id`. This ensures that transactions with same priority are not
        // equal, so they're not replaced in the set
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// Pending Transactions that are currently parked until their set baseFee becomes valid
struct ParkedTransactions<T: TransactionOrdering> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// All transactions that are currently parked due to their fee.
    parked_transactions: HashMap<TxHash, ParkedTransaction<T>>,
    /// Same transactions but sorted by their fee and priority
    sorted_transactions: BTreeSet<ParkedTransactionRef<T>>,
}

impl<T: TransactionOrdering> Default for ParkedTransactions<T> {
    fn default() -> Self {
        Self {
            id: 0,
            parked_transactions: Default::default(),
            sorted_transactions: Default::default(),
        }
    }
}

/// A transaction that is ready to be included in a block.
#[derive(Debug, Clone)]
pub(crate) struct ParkedTransaction<T: TransactionOrdering> {
    /// Reference to the actual transaction.
    transaction: PoolTransactionRef<T>,
    /// Tracks the transactions that get unlocked by this transaction.
    unlocks: Vec<TxHash>,
    /// Amount of required dependencies that are inherently provided
    requires_offset: usize,
}

/// A reference to a currently _parked_ transaction.
struct ParkedTransactionRef<T: TransactionOrdering> {
    /// Actual transaction.
    transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// Identifier that tags when transaction was submitted in the pool.
    submission_id: u64,
    /// The priority value assigned by the used `Ordering` function.
    priority: T::Priority,
    /// EIP-1559 Max base fee the caller is willing to pay.
    max_fee_per_gas: U256,
}

impl<T: TransactionOrdering> Eq for ParkedTransactionRef<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for ParkedTransactionRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for ParkedTransactionRef<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for ParkedTransactionRef<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares the `max_fee_per_gas` value of the transaction
        self.max_fee_per_gas
            .cmp(&other.max_fee_per_gas)
            .then_with(|| self.priority.cmp(&other.priority))
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// An iterator that returns transactions that can be executed on the current state.
pub struct TransactionsIterator<T: TransactionOrdering> {
    all: HashMap<TxHash, PendingTransaction<T>>,
    awaiting: HashMap<TxHash, (usize, PoolTransactionRef<T>)>,
    independent: BTreeSet<PoolTransactionRef<T>>,
    invalid: HashSet<TxHash>,
}

// == impl TransactionsIterator ==

impl<T: TransactionOrdering> TransactionsIterator<T> {
    /// Mark the transaction as invalid.
    ///
    /// As a consequence, all values that depend on the invalid one will be skipped.
    /// When given transaction is not in the pool it has no effect.
    pub(crate) fn mark_invalid(&mut self, tx: &Arc<ValidPoolTransaction<T::Transaction>>) {
        if let Some(invalid_transaction) = self.all.get(tx.hash()) {
            debug!(
                target: "txpool",
                "[{:?}] Marked as invalid",
                invalid_transaction.transaction.transaction.hash()
            );
            for hash in &invalid_transaction.unlocks {
                self.invalid.insert(*hash);
            }
        }
    }

    /// Depending on number of satisfied requirements insert given ref
    /// either to awaiting set or to best set.
    fn independent_or_awaiting(&mut self, satisfied: usize, tx_ref: PoolTransactionRef<T>) {
        if satisfied >= tx_ref.transaction.depends_on.len() {
            // If we have satisfied all deps insert to the best set
            self.independent.insert(tx_ref);
        } else {
            // otherwise we're still waiting for some deps
            self.awaiting.insert(*tx_ref.transaction.hash(), (satisfied, tx_ref));
        }
    }
}

impl<T: TransactionOrdering> BestTransactions for TransactionsIterator<T> {
    fn mark_invalid(&mut self, tx: &Self::Item) {
        TransactionsIterator::mark_invalid(self, tx)
    }
}

impl<T: TransactionOrdering> Iterator for TransactionsIterator<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let best = self.independent.iter().next_back()?.clone();
            let best = self.independent.take(&best)?;
            let hash = best.transaction.hash();

            // skip transactions that were marked as invalid
            if self.invalid.contains(hash) {
                debug!(
                    target: "txpool",
                    "[{:?}] skipping invalid transaction",
                    hash
                );
                continue
            }

            let ready =
                if let Some(ready) = self.all.get(hash).cloned() { ready } else { continue };

            // Insert transactions that just got unlocked.
            for hash in &ready.unlocks {
                // first check local awaiting transactions
                let res = if let Some((mut satisfied, tx_ref)) = self.awaiting.remove(hash) {
                    satisfied += 1;
                    Some((satisfied, tx_ref))
                    // then get from the pool
                } else {
                    self.all
                        .get(hash)
                        .map(|next| (next.requires_offset + 1, next.transaction.clone()))
                };
                if let Some((satisfied, tx_ref)) = res {
                    self.independent_or_awaiting(satisfied, tx_ref)
                }
            }

            return Some(best.transaction)
        }
    }
}
