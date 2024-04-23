use crate::{
    identifier::{SenderId, TransactionId},
    pool::size::SizeTracker,
    PoolTransaction, SubPoolLimit, ValidPoolTransaction, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
};
use fnv::FnvHashMap;
use smallvec::SmallVec;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeMap, BTreeSet},
    ops::{Bound::Unbounded, Deref},
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
#[derive(Debug, Clone)]
pub struct ParkedPool<T: ParkedOrd> {
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
    /// Keeps track of last submission id for each sender.
    ///
    /// This are sorted in Reverse order, so the last (highest) submission id is first, and the
    /// lowest(oldest) is the last.
    last_sender_submission: BTreeSet<SubmissionSenderId>,
    /// Keeps track of the number of transactions in the pool by the sender and the last submission
    /// id.
    sender_transaction_count: FnvHashMap<SenderId, SenderTransactionCount>,
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
    pub fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = *tx.id();
        assert!(
            !self.contains(&id),
            "transaction already included {:?}",
            self.get(&id).unwrap().transaction.transaction
        );
        let submission_id = self.next_id();

        // keep track of size
        self.size_of += tx.size();

        // update or create sender entry
        self.add_sender_count(tx.sender_id(), submission_id);
        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };

        self.by_id.insert(id, transaction.clone());
        self.best.insert(transaction);
    }

    /// Increments the count of transactions for the given sender and updates the tracked submission
    /// id.
    fn add_sender_count(&mut self, sender: SenderId, submission_id: u64) {
        match self.sender_transaction_count.entry(sender) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                // remove the __currently__ tracked submission id
                self.last_sender_submission
                    .remove(&SubmissionSenderId::new(sender, value.last_submission_id));

                value.count += 1;
                value.last_submission_id = submission_id;
            }
            Entry::Vacant(entry) => {
                entry
                    .insert(SenderTransactionCount { count: 1, last_submission_id: submission_id });
            }
        }
        // insert a new entry
        self.last_sender_submission.insert(SubmissionSenderId::new(sender, submission_id));
    }

    /// Decrements the count of transactions for the given sender.
    ///
    /// If the count reaches zero, the sender is removed from the map.
    ///
    /// Note: this does not update the tracked submission id for the sender, because we're only
    /// interested in the __last__ submission id when truncating the pool.
    fn remove_sender_count(&mut self, sender_id: SenderId) {
        let removed_sender = match self.sender_transaction_count.entry(sender_id) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                value.count -= 1;
                if value.count == 0 {
                    entry.remove()
                } else {
                    return
                }
            }
            Entry::Vacant(_) => {
                // This should never happen because the bisection between the two maps
                unreachable!("sender count not found {:?}", sender_id);
            }
        };

        // all transactions for this sender have been removed
        assert!(
            self.last_sender_submission
                .remove(&SubmissionSenderId::new(sender_id, removed_sender.last_submission_id)),
            "last sender transaction not found {sender_id:?}"
        );
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
        self.remove_sender_count(tx.transaction.sender_id());

        // keep track of size
        self.size_of -= tx.transaction.size();

        Some(tx.transaction.into())
    }

    /// Retrieves transactions by sender, using `SmallVec` to efficiently handle up to
    /// `TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER` transactions.
    pub(crate) fn get_txs_by_sender(
        &self,
        sender: SenderId,
    ) -> SmallVec<[TransactionId; TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER]> {
        self.by_id
            .range((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
            .map(|(tx_id, _)| *tx_id)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn get_senders_by_submission_id(
        &self,
    ) -> impl Iterator<Item = SubmissionSenderId> + '_ {
        self.last_sender_submission.iter().cloned()
    }

    /// Truncates the pool by removing transactions, until the given [SubPoolLimit] has been met.
    ///
    /// This is done by first ordering senders by the last time they have submitted a transaction
    ///
    /// Uses sender ids sorted by each sender's last submission id. Senders with older last
    /// submission ids are first. Note that _last_ submission ids are the newest submission id for
    /// that sender, so this sorts senders by the last time they submitted a transaction in
    /// descending order. Senders that have least recently submitted a transaction are first.
    ///
    /// Then, for each sender, all transactions for that sender are removed, until the pool limits
    /// have been met.
    ///
    /// Any removed transactions are returned.
    pub fn truncate_pool(
        &mut self,
        limit: SubPoolLimit,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if !self.exceeds(&limit) {
            // if we are below the limits, we don't need to drop anything
            return Vec::new()
        }

        let mut removed = Vec::new();

        while limit.is_exceeded(self.len(), self.size()) && !self.last_sender_submission.is_empty()
        {
            // NOTE: This will not panic due to `!last_sender_transaction.is_empty()`
            let sender_id = self.last_sender_submission.last().expect("not empty").sender_id;
            let list = self.get_txs_by_sender(sender_id);

            // Drop transactions from this sender until the pool is under limits
            for txid in list.into_iter().rev() {
                if let Some(tx) = self.remove_transaction(&txid) {
                    removed.push(tx);
                }

                if !self.exceeds(&limit) {
                    break
                }
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

    /// Returns true if the pool exceeds the given limit
    #[inline]
    pub(crate) fn exceeds(&self, limit: &SubPoolLimit) -> bool {
        limit.is_exceeded(self.len(), self.size())
    }

    /// Returns whether the pool is empty
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Returns `true` if the transaction with the given id is already included in this pool.
    pub(crate) fn contains(&self, id: &TransactionId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Retrieves a transaction with the given ID from the pool, if it exists.
    fn get(&self, id: &TransactionId) -> Option<&ParkedPoolTransaction<T>> {
        self.by_id.get(id)
    }

    /// Asserts that the bijection between `by_id` and `best` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(self.by_id.len(), self.best.len(), "by_id.len() != best.len()");

        assert_eq!(
            self.last_sender_submission.len(),
            self.sender_transaction_count.len(),
            "last_sender_transaction.len() != sender_to_last_transaction.len()"
        );
    }
}

impl<T: PoolTransaction> ParkedPool<BasefeeOrd<T>> {
    /// Returns all transactions that satisfy the given basefee.
    ///
    /// Note: this does _not_ remove the transactions
    #[allow(dead_code)]
    pub(crate) fn satisfy_base_fee_transactions(
        &self,
        basefee: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let ids = self.satisfy_base_fee_ids(basefee);
        let mut txs = Vec::with_capacity(ids.len());
        for id in ids {
            txs.push(self.get(&id).expect("transaction exists").transaction.clone().into());
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
            last_sender_submission: Default::default(),
            sender_transaction_count: Default::default(),
            size_of: Default::default(),
        }
    }
}

/// Keeps track of the number of transactions and the latest submission id for each sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SenderTransactionCount {
    count: u64,
    last_submission_id: u64,
}

/// Represents a transaction in this pool.
#[derive(Debug)]
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

/// Includes a [SenderId] and `submission_id`. This is used to sort senders by their last
/// submission id.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub(crate) struct SubmissionSenderId {
    /// The sender id
    pub(crate) sender_id: SenderId,
    /// The submission id
    pub(crate) submission_id: u64,
}

impl SubmissionSenderId {
    /// Creates a new [SubmissionSenderId] based on the [SenderId] and `submission_id`.
    const fn new(sender_id: SenderId, submission_id: u64) -> Self {
        Self { sender_id, submission_id }
    }
}

impl Ord for SubmissionSenderId {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for `submission_id`
        other.submission_id.cmp(&self.submission_id)
    }
}

impl PartialOrd for SubmissionSenderId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Helper trait used for custom `Ord` wrappers around a transaction.
///
/// This is effectively a wrapper for `Arc<ValidPoolTransaction>` with custom `Ord` implementation.
pub trait ParkedOrd:
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
pub struct BasefeeOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

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
pub struct QueuedOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

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
    use super::*;
    use crate::test_utils::{MockTransaction, MockTransactionFactory, MockTransactionSet};
    use reth_primitives::{address, TxType};
    use std::collections::HashSet;

    #[test]
    fn test_enforce_parked_basefee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx.clone());

        assert!(pool.contains(tx.id()));
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

        assert!(pool.contains(root_tx.id()));
        assert!(pool.contains(descendant_tx.id()));
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
            assert!(!pool2.contains(root_tx.id()));
            assert!(pool2.contains(descendant_tx.id()));
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

        let a_sender = address!("000000000000000000000000000000000000000a");
        let b_sender = address!("000000000000000000000000000000000000000b");
        let c_sender = address!("000000000000000000000000000000000000000c");
        let d_sender = address!("000000000000000000000000000000000000000d");

        // create a chain of transactions by sender A, B, C
        let mut tx_set = MockTransactionSet::dependent(a_sender, 0, 4, TxType::Eip1559);
        let a = tx_set.clone().into_vec();

        let b = MockTransactionSet::dependent(b_sender, 0, 3, TxType::Eip1559).into_vec();
        tx_set.extend(b.clone());

        // C has the same number of txs as B
        let c = MockTransactionSet::dependent(c_sender, 0, 3, TxType::Eip1559).into_vec();
        tx_set.extend(c.clone());

        let d = MockTransactionSet::dependent(d_sender, 0, 1, TxType::Eip1559).into_vec();
        tx_set.extend(d.clone());

        let all_txs = tx_set.into_vec();

        // just construct a list of all txs to add
        let expected_parked = vec![c[0].clone(), c[1].clone(), c[2].clone(), d[0].clone()]
            .into_iter()
            .map(|tx| (tx.sender(), tx.nonce()))
            .collect::<HashSet<_>>();

        // we expect the truncate operation to go through the senders with the most txs, removing
        // txs based on when they were submitted, removing the oldest txs first, until the pool is
        // not over the limit
        let expected_removed = vec![
            a[0].clone(),
            a[1].clone(),
            a[2].clone(),
            a[3].clone(),
            b[0].clone(),
            b[1].clone(),
            b[2].clone(),
        ]
        .into_iter()
        .map(|tx| (tx.sender(), tx.nonce()))
        .collect::<HashSet<_>>();

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
    fn test_truncate_parked_with_large_tx() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let default_limits = SubPoolLimit::default();

        // create a chain of transactions by sender A
        // make sure they are all one over half the limit
        let a_sender = address!("000000000000000000000000000000000000000a");

        // 2 txs, that should put the pool over the size limit but not max txs
        let a_txs = MockTransactionSet::dependent(a_sender, 0, 2, TxType::Eip1559)
            .into_iter()
            .map(|mut tx| {
                tx.set_size(default_limits.max_size / 2 + 1);
                tx
            })
            .collect::<Vec<_>>();

        // add all the transactions to the pool
        for tx in a_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        // truncate the pool, it should remove at least one transaction
        let removed = pool.truncate_pool(default_limits);
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn test_senders_by_submission_id() {
        // this test ensures that we evict from the pending pool by sender
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

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

        let all_txs = tx_set.into_vec();

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        // get senders by submission id - a4, b3, c3, d1, reversed
        let senders = pool.get_senders_by_submission_id().map(|s| s.sender_id).collect::<Vec<_>>();
        assert_eq!(senders.len(), 4);
        let expected_senders = vec![d_sender, c_sender, b_sender, a_sender]
            .into_iter()
            .map(|s| f.ids.sender_id(&s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(senders, expected_senders);

        // manually order the txs
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let all_txs = vec![d[0].clone(), b[0].clone(), c[0].clone(), a[0].clone()];

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        let senders = pool.get_senders_by_submission_id().map(|s| s.sender_id).collect::<Vec<_>>();
        assert_eq!(senders.len(), 4);
        let expected_senders = vec![a_sender, c_sender, b_sender, d_sender]
            .into_iter()
            .map(|s| f.ids.sender_id(&s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(senders, expected_senders);
    }
}
