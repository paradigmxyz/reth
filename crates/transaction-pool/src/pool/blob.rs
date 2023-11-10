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
    /// Keeps track of the current fees, so transaction priority can be calculated on insertion.
    pending_fees: PendingFees,
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

        // set transaction, which will also calculate priority based on current pending fees
        let transaction = BlobTransaction::new(tx, submission_id, &self.pending_fees);

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

    /// Resorts the transactions in the pool based on the pool's current [PendingFees].
    pub(crate) fn reprioritize(&mut self) {
        for tx in self.by_id.values_mut() {
            tx.update_priority(&self.pending_fees);
        }
    }

    /// Removes all transactions (and their descendants) which:
    ///  * have a `max_fee_per_blob_gas` greater than or equal to the given `blob_fee`, _and_
    ///  * have a `max_fee_per_gas` greater than or equal to the given `base_fee`
    ///
    /// This also sets the [PendingFees] for the pool, resorting transactions based on their
    /// updated priority.
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

        // set pending fees and reprioritize / resort
        self.pending_fees = pending_fees.clone();
        self.reprioritize();

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
            pending_fees: Default::default(),
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

impl<T: PoolTransaction> BlobTransaction<T> {
    /// Creates a new blob transaction, based on the pool transaction, submission id, and current
    /// pending fees.
    pub(crate) fn new(
        transaction: Arc<ValidPoolTransaction<T>>,
        submission_id: u64,
        pending_fees: &PendingFees,
    ) -> Self {
        let priority = blob_tx_priority(
            pending_fees.blob_fee,
            transaction.max_fee_per_blob_gas().unwrap_or_default(),
            pending_fees.base_fee as u128,
            transaction.max_fee_per_gas(),
        );
        let ord = BlobOrd { submission_id, priority };
        Self { transaction, ord }
    }

    /// Updates the priority for the transaction based on the current pending fees.
    pub(crate) fn update_priority(&mut self, pending_fees: &PendingFees) {
        self.ord.priority = blob_tx_priority(
            pending_fees.blob_fee,
            self.transaction.max_fee_per_blob_gas().unwrap_or_default(),
            pending_fees.base_fee as u128,
            self.transaction.max_fee_per_gas(),
        );
    }
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

/// The blob step function, attempting to compute the delta given the `max_tx_fee`, and
/// `current_fee`.
///
/// The `max_tx_fee` is the maximum fee that the transaction is willing to pay, this
/// would be the priority fee for the EIP1559 component of transaction fees, and the blob fee cap
/// for the blob component of transaction fees.
///
/// The `current_fee` is the current value of the fee, this would be the base fee for the EIP1559
/// component, and the blob fee (computed from the current head) for the blob component.
///
/// This is supposed to get the number of fee jumps required to get from the current fee to the fee
/// cap, or where the transaction would not be executable any more.
fn fee_delta(max_tx_fee: u128, current_fee: u128) -> f64 {
    // jumps = log1.125(txfee) - log1.125(basefee)
    // TODO: should we do this without f64?
    let jumps = (max_tx_fee as f64).log(1.125) - (current_fee as f64).log(1.125);
    // delta = sign(jumps) * log(abs(jumps))
    jumps.signum() * jumps.abs().log2()
}

/// Returns the priority for the transaction, based on the "delta" blob fee and priority fee.
fn blob_tx_priority(
    blob_fee_cap: u128,
    blob_fee: u128,
    max_priority_fee: u128,
    base_fee: u128,
) -> f64 {
    let delta_blob_fee = fee_delta(blob_fee_cap, blob_fee);
    let delta_priority_fee = fee_delta(max_priority_fee, base_fee);

    // priority = min(delta-basefee, delta-blobfee, 0)
    delta_blob_fee.min(delta_priority_fee).min(0.0)
}

#[derive(Debug, Clone)]
struct BlobOrd {
    /// Identifier that tags when transaction was submitted in the pool.
    pub(crate) submission_id: u64,
    // The priority for this transaction, calculated using the [`blob_tx_priority`] function,
    // taking into account both the blob and priority fee.
    pub(crate) priority: f64,
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
        other.priority.total_cmp(&self.priority)
    }
}

#[cfg(test)]
mod tests {
    use super::fee_delta;

    #[test]
    fn test_priority() {
        // Test vectors from:
        // <https://github.com/ethereum/go-ethereum/blob/e91cdb49beb4b2a3872b5f2548bf2d6559e4f561/core/txpool/blobpool/priority_test.go#L27-L49>
        let vectors = vec![
            (7u128, 10u128, 2f64),
            (17_200_000_000, 17_200_000_000, 0f64),
            (9_853_941_692, 11_085_092_510, 0f64),
            (11_544_106_391, 10_356_781_100, 0f64),
            (17_200_000_000, 7, -7f64),
            (7, 17_200_000_000, 7f64),
        ];

        for (base_fee, tx_fee, expected) in vectors {
            let actual = fee_delta(tx_fee, base_fee);
            println!("fee_delta({}, {}) = {}, expected: {}", tx_fee, base_fee, actual, expected);
            // assert_eq!(actual, expected, "fee_delta({}, {}) = {}", tx_fee, base_fee, actual);
        }
        // TODO: convert priority / delta into u64 methods to match vector
        panic!()
    }
}
