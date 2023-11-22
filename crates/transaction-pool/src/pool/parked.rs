use crate::{
    identifier::TransactionId, pool::size::SizeTracker, PoolTransaction, SubPoolLimit,
    ValidPoolTransaction,
};
use reth_primitives::Address;
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    ops::Deref,
    sync::Arc,
};

/// A pool of transactions that are currently parked and are waiting for external changes (e.g.
/// basefee, ancestor transactions, balance) that eventually move the transaction into the pending
/// pool.
///
/// This pool is a bijection: at all times each set (`best`, `by_id`) contains the same
/// transactions.
///
/// Note: This type is generic over [ParkedPool] which enforces that the underlying transaction type
/// is [ValidPoolTransaction] wrapped in an [Arc].
#[derive(Clone)]
pub(crate) struct ParkedPool<T: ParkedOrd> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions were submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, ParkedPoolTransaction<T>>,
    /// All transactions sorted by their order function.
    ///
    /// The higher, the better.
    best: BTreeSet<ParkedPoolTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`].
    size_of: SizeTracker,
}

// === impl ParkedPool ===

impl<T: ParkedOrd> ParkedPool<T> {
    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// If the transaction is already included.
    pub(crate) fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = *tx.id();
        assert!(
            !self.by_id.contains_key(&id),
            "transaction already included {:?}",
            self.by_id.contains_key(&id)
        );
        let submission_id = self.next_id();

        // keep track of size
        self.size_of += tx.size();

        // update or create sender entry
        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };

        self.by_id.insert(id, transaction.clone());
        self.best.insert(transaction);
    }

    /// Returns an iterator over all transactions in the pool
    pub(crate) fn all(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.by_id.values().map(|tx| tx.transaction.clone().into())
    }

    /// Removes the transaction from the pool
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // remove from queues
        let tx = self.by_id.remove(id)?;
        self.best.remove(&tx);

        // keep track of size
        self.size_of -= tx.transaction.size();

        Some(tx.transaction.into())
    }

    /// Get transactions by sender
    pub(crate) fn get_txs_by_sender(
        &self,
        sender: &Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.by_id
            .values()
            .filter(|tx| tx.transaction.sender() == *sender)
            .map(|tx| tx.transaction.clone().into())
            .collect()
    }

    /// Returns addresses sorted by their last submission id. Addresses with older last submission
    /// ids are first. Note that _last_ submission ids are the newest submission id for that
    /// sender, so this sorts senders by the last time they submitted a transaction in descending
    /// order. Senders that have least recently submitted a transaction are first.
    ///
    /// Similar to `Heartbeat` in Geth
    pub(crate) fn get_senders_by_submission_id(&self) -> Vec<Address> {
        // iterate through by_id, and get the last submission id for each sender
        let senders = self
            .by_id
            .iter()
            .fold(Vec::new(), |mut set: Vec<Reverse<(u64, Address)>>, (_, tx)| {
                if let Some(last) = set.last_mut() {
                    // sort by last
                    if last.0 .1 == tx.transaction.sender() {
                        if last.0 .0 < tx.submission_id {
                            // update last submission id
                            last.0 .0 = tx.submission_id;
                        }
                    } else {
                        // new entry
                        set.push(Reverse((tx.submission_id, tx.transaction.sender())));
                    }
                } else {
                    // first entry
                    set.push(Reverse((tx.submission_id, tx.transaction.sender())));
                }
                set
            })
            .into_iter()
            // sort by submission id
            .collect::<BinaryHeap<_>>();

        // sort s.t. senders with older submission ids are first
        senders.into_sorted_vec().into_iter().map(|Reverse((_, addr))| addr).collect()
    }

    /// Truncates the pool by dropping transactions, first dropping transactions from senders that
    /// have not recently submitted a transaction.
    pub(crate) fn truncate_pool(
        &mut self,
        limit: SubPoolLimit,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();

        let mut addresses = self.get_senders_by_submission_id();
        let queued = self.len();
        let mut drop = queued - limit.max_txs;

        while drop > 0 && !addresses.is_empty() {
            // SAFETY: This will not panic due to `!addresses.is_empty()`
            let addr = addresses.pop().unwrap();
            let mut list = self.get_txs_by_sender(&addr);

            // Drop all transactions if they are less than the overflow
            if list.len() <= drop {
                for tx in &list {
                    if let Some(tx) = self.remove_transaction(tx.id()) {
                        removed.push(tx);
                    }
                }
                drop -= list.len();
                continue
            }

            // Otherwise drop only last few transactions
            // SAFETY: This will not panic because `list.len() > drop`
            for tx in list.split_off(drop) {
                if let Some(tx) = self.remove_transaction(tx.id()) {
                    removed.push(tx);
                }
                drop -= 1;
            }
        }

        removed
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

    /// Returns `true` if the transaction with the given id is already included in this pool.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: &TransactionId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Asserts that the bijection between `by_id` and `best` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(self.by_id.len(), self.best.len(), "by_id.len() != best.len()");
    }
}

impl<T: PoolTransaction> ParkedPool<BasefeeOrd<T>> {
    /// Returns all transactions that satisfy the given basefee.
    ///
    /// Note: this does _not_ remove the transactions
    pub(crate) fn satisfy_base_fee_transactions(
        &self,
        basefee: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let ids = self.satisfy_base_fee_ids(basefee);
        let mut txs = Vec::with_capacity(ids.len());
        for id in ids {
            txs.push(self.by_id.get(&id).expect("transaction exists").transaction.clone().into());
        }
        txs
    }

    /// Returns all transactions that satisfy the given basefee.
    fn satisfy_base_fee_ids(&self, basefee: u64) -> Vec<TransactionId> {
        let mut transactions = Vec::new();
        {
            let mut iter = self.by_id.iter().peekable();

            while let Some((id, tx)) = iter.next() {
                if tx.transaction.transaction.max_fee_per_gas() < basefee as u128 {
                    // still parked -> skip descendant transactions
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

    /// Removes all transactions and their dependent transaction from the subpool that no longer
    /// satisfy the given basefee.
    ///
    /// Note: the transactions are not returned in a particular order.
    pub(crate) fn enforce_basefee(&mut self, basefee: u64) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let to_remove = self.satisfy_base_fee_ids(basefee);

        let mut removed = Vec::with_capacity(to_remove.len());
        for id in to_remove {
            removed.push(self.remove_transaction(&id).expect("transaction exists"));
        }

        removed
    }
}

impl<T: ParkedOrd> Default for ParkedPool<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            best: Default::default(),
            size_of: Default::default(),
        }
    }
}

/// Represents a transaction in this pool.
struct ParkedPoolTransaction<T: ParkedOrd> {
    /// Identifier that tags when transaction was submitted in the pool.
    submission_id: u64,
    /// Actual transaction.
    transaction: T,
}

impl<T: ParkedOrd> Clone for ParkedPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self { submission_id: self.submission_id, transaction: self.transaction.clone() }
    }
}

impl<T: ParkedOrd> Eq for ParkedPoolTransaction<T> {}

impl<T: ParkedOrd> PartialEq<Self> for ParkedPoolTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: ParkedOrd> PartialOrd<Self> for ParkedPoolTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> Ord for ParkedPoolTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by the transactions first, and only if two tx are equal this compares
        // the unique `submission_id`.
        // "better" transactions are Greater
        self.transaction
            .cmp(&other.transaction)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// Includes a [TransactionId] and `submission_id`, used for eviction.
#[derive(Debug, PartialEq, Eq, Clone)]
struct EvictionEntry {
    id: TransactionId,
    submission_id: u64,
}

impl Ord for EvictionEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.submission_id.cmp(&other.submission_id)
    }
}

impl PartialOrd for EvictionEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Helper trait used for custom `Ord` wrappers around a transaction.
///
/// This is effectively a wrapper for `Arc<ValidPoolTransaction>` with custom `Ord` implementation.
pub(crate) trait ParkedOrd:
    Ord
    + Clone
    + From<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Into<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Deref<Target = Arc<ValidPoolTransaction<Self::Transaction>>>
{
    /// The wrapper transaction type.
    type Transaction: PoolTransaction;
}

/// Helper macro to implement necessary conversions for `ParkedOrd` trait
macro_rules! impl_ord_wrapper {
    ($name:ident) => {
        impl<T: PoolTransaction> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<T: PoolTransaction> Eq for $name<T> {}

        impl<T: PoolTransaction> PartialEq<Self> for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == Ordering::Equal
            }
        }

        impl<T: PoolTransaction> PartialOrd<Self> for $name<T> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl<T: PoolTransaction> Deref for $name<T> {
            type Target = Arc<ValidPoolTransaction<T>>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<T: PoolTransaction> ParkedOrd for $name<T> {
            type Transaction = T;
        }

        impl<T: PoolTransaction> From<Arc<ValidPoolTransaction<T>>> for $name<T> {
            fn from(value: Arc<ValidPoolTransaction<T>>) -> Self {
                Self(value)
            }
        }

        impl<T: PoolTransaction> From<$name<T>> for Arc<ValidPoolTransaction<T>> {
            fn from(value: $name<T>) -> Arc<ValidPoolTransaction<T>> {
                value.0
            }
        }
    };
}

/// A new type wrapper for [`ValidPoolTransaction`]
///
/// This sorts transactions by their base fee.
///
/// Caution: This assumes all transaction in the `BaseFee` sub-pool have a fee value.
#[derive(Debug)]
pub(crate) struct BasefeeOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(BasefeeOrd);

impl<T: PoolTransaction> Ord for BasefeeOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.transaction.max_fee_per_gas().cmp(&other.0.transaction.max_fee_per_gas())
    }
}

/// A new type wrapper for [`ValidPoolTransaction`]
///
/// This sorts transactions by their distance.
///
/// `Queued` transactions are transactions that are currently blocked by other parked (basefee,
/// queued) or missing transactions.
///
/// The primary order function always compares the transaction costs first. In case these
/// are equal, it compares the timestamps when the transactions were created.
#[derive(Debug)]
pub(crate) struct QueuedOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(QueuedOrd);

// TODO: temporary solution for ordering the queued pool.
impl<T: PoolTransaction> Ord for QueuedOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher price is better
        self.max_fee_per_gas().cmp(&self.max_fee_per_gas()).then_with(||
            // Lower timestamp is better
            other.timestamp.cmp(&self.timestamp))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use reth_primitives::address;

    use super::*;
    use crate::test_utils::{MockTransaction, MockTransactionFactory};

    #[test]
    fn test_enforce_parked_basefee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx.clone());

        assert!(pool.by_id.contains_key(tx.id()));
        assert_eq!(pool.len(), 1);

        let removed = pool.enforce_basefee(u64::MAX);
        assert!(removed.is_empty());

        let removed = pool.enforce_basefee((tx.max_fee_per_gas() - 1) as u64);
        assert_eq!(removed.len(), 1);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_enforce_parked_basefee_descendant() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let t = MockTransaction::eip1559().inc_price_by(10);
        let root_tx = f.validated_arc(t.clone());
        pool.add_transaction(root_tx.clone());

        let descendant_tx = f.validated_arc(t.inc_nonce().decr_price());
        pool.add_transaction(descendant_tx.clone());

        assert!(pool.by_id.contains_key(root_tx.id()));
        assert!(pool.by_id.contains_key(descendant_tx.id()));
        assert_eq!(pool.len(), 2);

        let removed = pool.enforce_basefee(u64::MAX);
        assert!(removed.is_empty());
        assert_eq!(pool.len(), 2);
        // two dependent tx in the pool with decreasing fee

        {
            // TODO: test change might not be intended, re review
            let mut pool2 = pool.clone();
            let removed = pool2.enforce_basefee(root_tx.max_fee_per_gas() as u64);
            assert_eq!(removed.len(), 1);
            assert_eq!(pool2.len(), 1);
            // root got popped - descendant should be skipped
            assert!(!pool2.by_id.contains_key(root_tx.id()));
            assert!(pool2.by_id.contains_key(descendant_tx.id()));
        }

        // remove root transaction via descendant tx fee
        let removed = pool.enforce_basefee(descendant_tx.max_fee_per_gas() as u64);
        assert_eq!(removed.len(), 2);
        assert!(pool.is_empty());
    }

    #[test]
    fn truncate_parked_by_submission_id() {
        // this test ensures that we evict from the pending pool by sender
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        let a = address!("000000000000000000000000000000000000000a");
        let b = address!("000000000000000000000000000000000000000b");
        let c = address!("000000000000000000000000000000000000000c");
        let d = address!("000000000000000000000000000000000000000d");

        // TODO: make creating these mock tx chains easier
        // create a chain of transactions by sender A, B, C
        let a1 = MockTransaction::eip1559().with_sender(a);
        let a2 = a1.next();
        let a3 = a2.next();
        let a4 = a3.next();

        let b1 = MockTransaction::eip1559().with_sender(b);
        let b2 = b1.next();
        let b3 = b2.next();

        // C has the same number of txs as B
        let c1 = MockTransaction::eip1559().with_sender(c);
        let c2 = c1.next();
        let c3 = c2.next();

        let d1 = MockTransaction::eip1559().with_sender(d);

        // just construct a list of all txs to add
        let expected_parked = vec![c1.clone(), c2.clone(), c3.clone(), d1.clone()]
            .into_iter()
            .map(|tx| (tx.sender(), tx.nonce()))
            .collect::<HashSet<_>>();

        // we expect the truncate operation to go through the senders with the most txs, removing
        // txs based on when they were submitted, removing the oldest txs first, until the pool is
        // not over the limit
        let expected_removed = vec![
            a1.clone(),
            a2.clone(),
            a3.clone(),
            a4.clone(),
            b1.clone(),
            b2.clone(),
            b3.clone(),
        ]
        .into_iter()
        .map(|tx| (tx.sender(), tx.nonce()))
        .collect::<HashSet<_>>();
        let all_txs = vec![a1, a2, a3, a4, b1, b2, b3, c1, c2, c3, d1];

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        // we should end up with the most recently submitted transactions
        let pool_limit = SubPoolLimit { max_txs: 4, max_size: usize::MAX };

        // truncate the pool
        let removed = pool.truncate_pool(pool_limit);
        assert_eq!(removed.len(), expected_removed.len());

        // get the inner txs from the removed txs
        let removed =
            removed.into_iter().map(|tx| (tx.sender(), tx.nonce())).collect::<HashSet<_>>();
        assert_eq!(removed, expected_removed);

        // get the parked pool
        let parked = pool.all().collect::<Vec<_>>();
        assert_eq!(parked.len(), expected_parked.len());

        // get the inner txs from the parked txs
        let parked = parked.into_iter().map(|tx| (tx.sender(), tx.nonce())).collect::<HashSet<_>>();
        assert_eq!(parked, expected_parked);
    }

    #[test]
    fn test_senders_by_submission_id() {
        // this test ensures that we evict from the pending pool by sender
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        let a = address!("000000000000000000000000000000000000000a");
        let b = address!("000000000000000000000000000000000000000b");
        let c = address!("000000000000000000000000000000000000000c");
        let d = address!("000000000000000000000000000000000000000d");

        // create a chain of transactions by sender A, B, C
        let a1 = MockTransaction::eip1559().with_sender(a);
        let a2 = a1.next();
        let a3 = a2.next();
        let a4 = a3.next();

        let b1 = MockTransaction::eip1559().with_sender(b);
        let b2 = b1.next();
        let b3 = b2.next();

        // C has the same number of txs as B
        let c1 = MockTransaction::eip1559().with_sender(c);
        let c2 = c1.next();
        let c3 = c2.next();

        let d1 = MockTransaction::eip1559().with_sender(d);

        let all_txs = vec![
            a1.clone(),
            a2.clone(),
            a3.clone(),
            a4.clone(),
            b1.clone(),
            b2.clone(),
            b3.clone(),
            c1.clone(),
            c2.clone(),
            c3.clone(),
            d1.clone(),
        ];

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        // get senders by submission id - a4, b3, c3, d1, reversed
        let senders = pool.get_senders_by_submission_id();
        assert_eq!(senders.len(), 4);
        assert_eq!(senders, vec![d, c, b, a]);

        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let all_txs = vec![a1, b1, c1, d1, a2, b2, c2, a3, b3, c3, a4];

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        let senders = pool.get_senders_by_submission_id();
        assert_eq!(senders.len(), 4);
        assert_eq!(senders, vec![a, c, b, d]);
    }
}
