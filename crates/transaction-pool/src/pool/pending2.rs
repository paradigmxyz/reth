use crate::{
    identifier::TransactionId, pool::pending::PoolTransactionRef, TransactionOrdering,
    ValidPoolTransaction,
};
use fnv::FnvHashMap;
use reth_primitives::rpc::TxHash;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
};

pub(crate) struct PendingPool<T: TransactionOrdering> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// All Transactions that are currently inside the pool grouped by their hash.
    by_hash: HashMap<TxHash, Arc<PendingTransaction<T>>>,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, Arc<PendingTransaction<T>>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PendingTransactionRef<T>>,
}

// === impl PendingPool ===

impl<T: TransactionOrdering> PendingPool<T> {
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
            all: self.by_hash.clone(),
            independent: self.independent_transactions.clone(),
            awaiting: Default::default(),
            invalid: Default::default(),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        id
    }

    /// Returns the transaction for the hash if it's in the ready pool but not yet mined.
    pub(crate) fn get(&self, hash: &TxHash) -> Option<Arc<PendingTransaction<T>>> {
        self.by_hash.get(hash).cloned()
    }
}

/// A transaction that is ready to be included in a block.
pub(crate) struct PendingTransaction<T: TransactionOrdering> {
    /// Reference to the actual transaction.
    transaction: PendingTransactionRef<T>,
    /// Tracks the transaction that gets unlocked by this transaction.
    unlocks: Option<TransactionId>,
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
        Self { transaction: self.transaction.clone(), unlocks: self.unlocks.clone() }
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

/// An iterator that returns transactions that can be executed on the current state.
pub struct TransactionsIterator<T: TransactionOrdering> {
    all: HashMap<TxHash, Arc<PendingTransaction<T>>>,
    independent: BTreeSet<PendingTransactionRef<T>>,
    awaiting: HashMap<TxHash, PoolTransactionRef<T>>,
    invalid: HashSet<TxHash>,
}
