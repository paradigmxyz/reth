use crate::{
    identifier::TransactionId,
    TransactionOrdering, ValidPoolTransaction,
};
use fnv::FnvHashMap;

use std::{
    cmp::Ordering,
    collections::{BTreeSet},
    sync::Arc,
};

/// A Sub-Pool that currently holds transactions that violate the dynamic fee requirement
pub(crate) struct BaseFeePool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: Arc<T>,
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: FnvHashMap<TransactionId, Arc<BasFeeTransaction<T>>>,
    /// All transactions sorted by their priority function.
    best: BTreeSet<Arc<BasFeeTransaction<T>>>,
}

// === impl BaseFeePool ===

impl<T: TransactionOrdering> BaseFeePool<T> {
    /// Create a new pool instance.
    pub(crate) fn new(ordering: Arc<T>) -> Self {
        Self { ordering, submission_id: 0, by_id: Default::default(), best: Default::default() }
    }

    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// if the transaction is already included
    pub(crate) fn add_transaction(&mut self, _tx: Arc<ValidPoolTransaction<T::Transaction>>) {}

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }
}

/// The transaction type of this pool
struct BasFeeTransaction<T: TransactionOrdering> {
    /// Identifier that tags when transaction was submitted in the pool.
    submission_id: u64,
    /// Actual transaction.
    transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// The priority value assigned by the used `Ordering` function.
    priority: T::Priority,
    /// The base fee
    base_fee: T::Priority,
}

impl<T: TransactionOrdering> Eq for BasFeeTransaction<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for BasFeeTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for BasFeeTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for BasFeeTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `transaction_id`.
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.transaction.id().cmp(other.transaction.id()))
    }
}
