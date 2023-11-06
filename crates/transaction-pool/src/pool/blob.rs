#![allow(dead_code, unused)]
use crate::{
    identifier::TransactionId, pool::size::SizeTracker, traits::BestTransactionsAttributes,
    PoolTransaction, ValidPoolTransaction,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::txpool::PendingFees;

/// A set of validated blob transactions in the pool that are __not pending__.
///
/// The purpose of this pool is keep track of blob transactions that are queued and to evict the
/// worst blob transactions once the sub-pool is full.
///
/// This expects that certain constraints are met:
///   - blob transactions are always gap less
pub(crate) struct BlobTransactions<T: PoolTransaction> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions were submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, BlobTransaction<T>>,
    /// _All_ transactions sorted by blob priority.
    all: BTreeSet<BlobTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`].
    size_of: SizeTracker,
}

// === impl BlobTransactions ===

impl<T: PoolTransaction> BlobTransactions<T> {
    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    ///   - If the transaction is not a blob tx.
    ///   - If the transaction is already included.
    pub(crate) fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T>>) {
        assert!(tx.is_eip4844(), "transaction is not a blob tx");
        let id = *tx.id();
        assert!(
            !self.by_id.contains_key(&id),
            "transaction already included {:?}",
            self.by_id.contains_key(&id)
        );
        let submission_id = self.next_id();

        // keep track of size
        self.size_of += tx.size();

        let ord = BlobOrd { submission_id };
        let transaction = BlobTransaction { ord, transaction: tx };

        self.by_id.insert(id, transaction.clone());
        self.all.insert(transaction);
    }

    /// Removes the transaction from the pool
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        // remove from queues
        let tx = self.by_id.remove(id)?;

        self.all.remove(&tx);

        // keep track of size
        self.size_of -= tx.transaction.size();

        Some(tx.transaction)
    }

    /// Returns all transactions that satisfy the given basefee and blob_fee.
    pub(crate) fn satisfy_attributes(
        &self,
        best_transactions_attributes: BestTransactionsAttributes,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        Vec::new()
    }

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// The reported size of all transactions in this pool.
    pub(crate) fn size(&self) -> usize {
        self.size_of.into()
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Returns whether the pool is empty
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Returns all transactions which:
    ///  * have a `max_fee_per_blob_gas` greater than or equal to the given `blob_fee`, _and_
    ///  * have a `max_fee_per_gas` greater than or equal to the given `base_fee`
    fn satisfy_pending_fee_ids(&self, pending_fees: &PendingFees) -> Vec<TransactionId> {
        let mut transactions = Vec::new();
        {
            let mut iter = self.by_id.iter().peekable();

            while let Some((id, tx)) = iter.next() {
                if tx.transaction.max_fee_per_blob_gas() < Some(pending_fees.blob_fee) ||
                    tx.transaction.max_fee_per_gas() < pending_fees.base_fee as u128
                {
                    // still parked in blob pool -> skip descendant transactions
                    'this: while let Some((peek, _)) = iter.peek() {
                        if peek.sender != id.sender {
                            break 'this
                        }
                        iter.next();
                    }
                } else {
                    transactions.push(*id);
                }
            }
        }
        transactions
    }

    /// Removes all transactions (and their descendants) which:
    ///  * have a `max_fee_per_blob_gas` greater than or equal to the given `blob_fee`, _and_
    ///  * have a `max_fee_per_gas` greater than or equal to the given `base_fee`
    ///
    /// Note: the transactions are not returned in a particular order.
    pub(crate) fn enforce_pending_fees(
        &mut self,
        pending_fees: &PendingFees,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let to_remove = self.satisfy_pending_fee_ids(pending_fees);

        let mut removed = Vec::with_capacity(to_remove.len());
        for id in to_remove {
            removed.push(self.remove_transaction(&id).expect("transaction exists"));
        }

        removed
    }

    /// Returns `true` if the transaction with the given id is already included in this pool.
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn contains(&self, id: &TransactionId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Asserts that the bijection between `by_id` and `all` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(self.by_id.len(), self.all.len(), "by_id.len() != all.len()");
    }
}

impl<T: PoolTransaction> Default for BlobTransactions<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            all: Default::default(),
            size_of: Default::default(),
        }
    }
}

/// A transaction that is ready to be included in a block.
struct BlobTransaction<T: PoolTransaction> {
    /// Actual blob transaction.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// The value that determines the order of this transaction.
    ord: BlobOrd,
}

impl<T: PoolTransaction> Clone for BlobTransaction<T> {
    fn clone(&self) -> Self {
        Self { transaction: self.transaction.clone(), ord: self.ord.clone() }
    }
}

impl<T: PoolTransaction> Eq for BlobTransaction<T> {}

impl<T: PoolTransaction> PartialEq<Self> for BlobTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: PoolTransaction> PartialOrd<Self> for BlobTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: PoolTransaction> Ord for BlobTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ord.cmp(&other.ord)
    }
}

#[derive(Debug, Clone)]
struct BlobOrd {
    /// Identifier that tags when transaction was submitted in the pool.
    pub(crate) submission_id: u64,
    // TODO(mattsse): add ord values
}

impl Eq for BlobOrd {}

impl PartialEq<Self> for BlobOrd {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd<Self> for BlobOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BlobOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        other.submission_id.cmp(&self.submission_id)
    }
}
