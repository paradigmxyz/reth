use crate::{TransactionOrdering, ValidPoolTransaction};
use reth_primitives::rpc::TxHash;
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

pub(crate) struct QueuedPool<T: TransactionOrdering> {
    /// All Transactions that are currently inside the pool.
    by_hash: HashMap<TxHash, SubPoolTransaction<T>>,
    /// All transactions sorted by their priority function
    transactions: BTreeSet<SubPoolTransaction<T>>,
}

/// A reference to a transaction in the pool
pub(crate) struct SubPoolTransaction<T: TransactionOrdering> {
    /// Actual transaction.
    pub(crate) transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// The priority value assigned by the used `Ordering` function.
    pub(crate) priority: T::Priority,
}

impl<T: TransactionOrdering> Eq for SubPoolTransaction<T> {}

impl<T: TransactionOrdering> PartialEq<Self> for SubPoolTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TransactionOrdering> PartialOrd<Self> for SubPoolTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TransactionOrdering> Ord for SubPoolTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `transaction_id`.
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.transaction.id().cmp(other.transaction.id()))
    }
}
