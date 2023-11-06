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

// When the pool eventually reaches saturation, some old transactions - that may never execute -
// will need to be evicted in favor of newer ones. The eviction strategy is quite complex:
//
//   - Exceeding capacity evicts the highest-nonce of the account with the lowest paying blob
//   transaction anywhere in the pooled nonce-sequence, as that tx would be executed the furthest
//   in the future and is thus blocking anything after it. The smallest is deliberately not evicted
//   to avoid a nonce-gap.
//
//   - Analogously, if the pool is full, the consideration price of a new tx for evicting an old
//   one is the smallest price in the entire nonce-sequence of the account. This avoids malicious
//   users DoSing the pool with seemingly high paying transactions hidden behind a low-paying
//   blocked one.
//
//   - Since blob transactions have 3 price parameters: execution tip, execution fee cap and data
//   fee cap, there's no singular parameter to create a total price ordering on. What's more, since
//   the base fee and blob fee can move independently of one another, there's no pre-defined way to
//   combine them into a stable order either. This leads to a multi-dimensional problem to solve
//   after every block.
//
//   - The first observation is that comparing 1559 base fees or 4844 blob fees needs to happen in
//   the context of their dynamism. Since these fees jump up or down in ~1.125 multipliers (at max)
//   across blocks, comparing fees in two transactions should be based on log1.125(fee) to
//   eliminate noise.
//
//   - The second observation is that the basefee and blobfee move independently, so there's no way
//   to split mixed txs on their own (A has higher base fee, B has higher blob fee). Rather than
//   look at the absolute fees, the useful metric is the max time it can take to exceed the
//   transaction's fee caps. Specifically, we're interested in the number of jumps needed to go
//   from the current fee to the transaction's cap:
//
//     jumps = log1.125(txfee) - log1.125(basefee)
//
//   - The third observation is that the base fee tends to hover around rather than swing wildly.
//   The number of jumps needed from the current fee starts to get less relevant the higher it is.
//   To remove the noise here too, the pool will use log(jumps) as the delta for comparing
//   transactions.
//
//     delta = sign(jumps) * log(abs(jumps))
//
//   - To establish a total order, we need to reduce the dimensionality of the two base fees (log
//   jumps) to a single value. The interesting aspect from the pool's perspective is how fast will
//   a tx get executable (fees going down, crossing the smaller negative jump counter) or
//   non-executable (fees going up, crossing the smaller positive jump counter). As such, the pool
//   cares only about the min of the two delta values for eviction priority.
//
//     priority = min(delta-basefee, delta-blobfee)
//
//   - The above very aggressive dimensionality and noise reduction should result in transaction
//   being grouped into a small number of buckets, the further the fees the larger the buckets.
//   This is good because it allows us to use the miner tip meaningfully as a splitter.
//
//   - For the scenario where the pool does not contain non-executable blob txs anymore, it does
//   not make sense to grant a later eviction priority to txs with high fee caps since it could
//   enable pool wars. As such, any positive priority will be grouped together.
//
//     priority = min(delta-basefee, delta-blobfee, 0)
//
// Optimisation tradeoffs:
//
//   - Eviction relies on 3 fee minimums per account (exec tip, exec cap and blob cap). Maintaining
//   these values across all transactions from the account is problematic as each transaction
//   replacement or inclusion would require a rescan of all other transactions to recalculate the
//   minimum. Instead, the pool maintains a rolling minimum across the nonce range. Updating all
//   the minimums will need to be done only starting at the swapped in/out nonce and leading up to
//   the first no-change.


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
