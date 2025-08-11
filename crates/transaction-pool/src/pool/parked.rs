use crate::{
    identifier::{SenderId, TransactionId},
    pool::size::SizeTracker,
    PoolTransaction, SubPoolLimit, ValidPoolTransaction, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
};
use smallvec::SmallVec;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ops::{Bound::Unbounded, Deref},
    sync::Arc,
};

/// A pool of transactions that are currently parked and are waiting for external changes (e.g.
/// basefee, ancestor transactions, balance) that eventually move the transaction into the pending
/// pool.
///
/// Note: This type is generic over [`ParkedPool`] which enforces that the underlying transaction
/// type is [`ValidPoolTransaction`] wrapped in an [Arc].
#[derive(Debug, Clone)]
pub struct ParkedPool<T: ParkedOrd> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions were submitted to the pool.
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: BTreeMap<TransactionId, ParkedPoolTransaction<T>>,
    /// Bitmap 1: [sender_id: u64][count: u64] - tracks transaction count per sender
    sender_id_count: Vec<SenderCount>,
    /// Bitmap 2: [sender_id: u64][last_submission_id: u64] - tracks last submission per sender
    sender_id_last_submission: Vec<SenderSubmission>,

    // Only contains senders that have transactions
    submission_order: Vec<SenderSubmission>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`reth_primitives_traits::InMemorySize::size`].
    size_of: SizeTracker,
}

impl<T: ParkedOrd> ParkedPool<T> {
    /// Returns an iterator over sender submissions, ordered by last submission id descending.
    pub fn get_senders_by_submission_id(&self) -> impl Iterator<Item = SenderSubmission> + '_ {
        let mut submissions = self.sender_id_last_submission.clone();
        // Sort by submission_id descending (most recent first)
        submissions.sort_by_key(|ss| std::cmp::Reverse(ss.submission_id()));
        submissions.into_iter()
    }
}

/// Packed 128-bit value: [sender_id: u64][count: u64]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SenderCount(u128);

impl SenderCount {
    /// Create a new SenderCount from sender_id and count
    const fn new(sender_id: SenderId, count: u64) -> Self {
        Self(((sender_id.as_u64()) as u128) << 64 | count as u128)
    }

    /// Extract sender_id from packed value
    const fn sender_id(self) -> SenderId {
        SenderId::from_u64((self.0 >> 64) as u64)
    }

    /// Extract count from packed value
    const fn count(self) -> u64 {
        self.0 as u64
    }
}

impl Ord for SenderCount {
    fn cmp(&self, other: &Self) -> Ordering {
        // Primary sort by sender_id, secondary by count
        self.sender_id().cmp(&other.sender_id()).then_with(|| self.count().cmp(&other.count()))
    }
}

impl PartialOrd for SenderCount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Packed 128-bit value: [sender_id: u64][last_submission_id: u64]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderSubmission(u128);

impl SenderSubmission {
    /// Create a new SenderSubmission from sender_id and submission_id
    const fn new(sender_id: SenderId, submission_id: u64) -> Self {
        Self(((sender_id.as_u64()) as u128) << 64 | submission_id as u128)
    }

    /// Extract sender_id from packed value
    const fn sender_id(self) -> SenderId {
        SenderId::from_u64((self.0 >> 64) as u64)
    }

    /// Extract submission_id from packed value
    const fn submission_id(self) -> u64 {
        self.0 as u64
    }
}

impl Ord for SenderSubmission {
    fn cmp(&self, other: &Self) -> Ordering {
        // For truncation purposes, we want reverse order by submission_id
        // when sender_ids are the same
        self.sender_id()
            .cmp(&other.sender_id())
            .then_with(|| other.submission_id().cmp(&self.submission_id()))
    }
}

impl PartialOrd for SenderSubmission {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
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

        self.by_id.insert(id, transaction);
    }

    /// Increments the count of transactions for the given sender and updates the tracked submission
    /// id.
    fn add_sender_count(&mut self, sender: SenderId, submission_id: u64) {
        let count_pos = self.sender_id_count.binary_search_by_key(&sender, |sc| sc.sender_id());
        let submission_pos =
            self.sender_id_last_submission.binary_search_by_key(&sender, |ss| ss.sender_id());

        let new_submission = SenderSubmission::new(sender, submission_id);

        match count_pos {
            Ok(idx) => {
                // Sender exists - increment count
                let current_count = self.sender_id_count[idx].count();
                self.sender_id_count[idx] = SenderCount::new(sender, current_count + 1);

                if let Ok(sub_idx) = submission_pos {
                    let old_submission = self.sender_id_last_submission[sub_idx];
                    self.sender_id_last_submission[sub_idx] = new_submission;

                    // Update submission_order Vec
                    self.update_submission_order(old_submission, new_submission);
                }
            }
            Err(idx) => {
                // New sender
                self.sender_id_count.insert(idx, SenderCount::new(sender, 1));
                let sub_idx = submission_pos.unwrap_err();
                self.sender_id_last_submission.insert(sub_idx, new_submission);

                // Add to submission_order in correct position
                self.insert_submission_order(new_submission);
            }
        }
    }

    fn update_submission_order(
        &mut self,
        old_submission: SenderSubmission,
        new_submission: SenderSubmission,
    ) {
        // Remove old entry
        if let Ok(idx) = self
            .submission_order
            .binary_search_by_key(&old_submission.submission_id(), |ss| ss.submission_id())
        {
            // Handle multiple entries with same submission_id
            let start = idx;
            let mut end = idx;
            while end < self.submission_order.len() &&
                self.submission_order[end].submission_id() == old_submission.submission_id()
            {
                end += 1;
            }

            // Find exact match
            for i in start..end {
                if self.submission_order[i].sender_id() == old_submission.sender_id() {
                    self.submission_order.remove(i);
                    break;
                }
            }
        }

        // Insert new entry
        self.insert_submission_order(new_submission);
    }

    fn insert_submission_order(&mut self, submission: SenderSubmission) {
        self.submission_order.push(submission);
    }

    /// Decrements the count of transactions for the given sender.
    ///
    /// If the count reaches zero, the sender is removed from the map.
    ///
    /// Note: this does not update the tracked submission id for the sender, because we're only
    /// interested in the __last__ submission id when truncating the pool.
    fn remove_sender_count(&mut self, sender_id: SenderId) {
        let count_pos = self.sender_id_count.binary_search_by_key(&sender_id, |sc| sc.sender_id());
        let submission_pos =
            self.sender_id_last_submission.binary_search_by_key(&sender_id, |ss| ss.sender_id());

        match count_pos {
            Ok(idx) => {
                let current_count = self.sender_id_count[idx].count();

                if current_count == 1u64 {
                    // Remove sender completely
                    self.sender_id_count.remove(idx);
                    if let Ok(sub_idx) = submission_pos {
                        let old_submission = self.sender_id_last_submission[sub_idx];
                        self.sender_id_last_submission.remove(sub_idx);

                        // Remove from submission_order
                        self.remove_from_submission_order(old_submission);
                    }
                } else {
                    // Just decrement count
                    self.sender_id_count[idx] = SenderCount::new(sender_id, current_count - 1);
                }
            }
            Err(_) => {
                unreachable!("sender count not found {:?}", sender_id);
            }
        }
    }

    fn remove_from_submission_order(&mut self, submission: SenderSubmission) {
        if let Ok(idx) = self
            .submission_order
            .binary_search_by_key(&submission.submission_id(), |ss| ss.submission_id())
        {
            // Handle multiple entries with same submission_id
            let start = idx;
            let mut end = idx;
            while end < self.submission_order.len() &&
                self.submission_order[end].submission_id() == submission.submission_id()
            {
                end += 1;
            }

            // Find exact match and remove
            for i in start..end {
                if self.submission_order[i].sender_id() == submission.sender_id() {
                    self.submission_order.swap_remove(i);
                    break;
                }
            }
        }
    }

    /// Returns an iterator over all transactions in the pool
    pub(crate) fn all(
        &self,
    ) -> impl ExactSizeIterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.by_id.values().map(|tx| tx.transaction.clone().into())
    }

    /// Removes the transaction from the pool
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // remove from queues
        let tx = self.by_id.remove(id)?;
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

    fn get_sender_count(&self, sender_id: SenderId) -> u64 {
        self.sender_id_count
            .binary_search_by_key(&sender_id, |sc| sc.sender_id())
            .map(|idx| self.sender_id_count[idx].count())
            .unwrap_or(0)
    }

    /// Truncates the pool by removing transactions, until the given [`SubPoolLimit`] has been met.
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
            return Vec::new();
        }

        self.submission_order.sort_by_key(|ss| ss.submission_id());

        let mut removed = Vec::new();
        let mut processed_senders = 0;

        while self.exceeds(&limit) && processed_senders < self.submission_order.len() {
            let submission = self.submission_order[processed_senders];
            let sender_id = submission.sender_id();

            let sender_txs = self.get_txs_by_sender(sender_id);

            for &tx_id in sender_txs.iter().rev() {
                if let Some(tx) = self.remove_transaction(&tx_id) {
                    removed.push(tx);
                }

                if !self.exceeds(&limit) {
                    break;
                }
            }

            processed_senders += 1;
        }

        if processed_senders > 0 {
            self.submission_order.drain(0..processed_senders);
        }

        removed
    }

    const fn next_id(&mut self) -> u64 {
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

    /// Asserts that all subpool invariants
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(
            self.sender_id_last_submission.len(),
            self.sender_id_count.len(),
            "sender_id_last_submission.len() != sender_id_count.len()"
        );
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
        let ids = self.satisfy_base_fee_ids(basefee as u128);
        let mut txs = Vec::with_capacity(ids.len());
        for id in ids {
            txs.push(self.get(&id).expect("transaction exists").transaction.clone().into());
        }
        txs
    }

    /// Returns all transactions that satisfy the given basefee.
    fn satisfy_base_fee_ids(&self, basefee: u128) -> Vec<TransactionId> {
        let mut transactions = Vec::new();
        {
            let mut iter = self.by_id.iter().peekable();

            while let Some((id, tx)) = iter.next() {
                if tx.transaction.transaction.max_fee_per_gas() < basefee {
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
        let to_remove = self.satisfy_base_fee_ids(basefee as u128);

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
            sender_id_count: Default::default(),
            sender_id_last_submission: Default::default(),
            submission_order: Vec::new(),
            size_of: Default::default(),
        }
    }
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

impl<T: PoolTransaction> Ord for QueuedOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher fee is better
        self.max_fee_per_gas().cmp(&other.max_fee_per_gas()).then_with(||
            // Lower timestamp is better
            other.timestamp.cmp(&self.timestamp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockTransaction, MockTransactionFactory, MockTransactionSet};
    use alloy_consensus::{Transaction, TxType};
    use alloy_primitives::address;
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

        let a_sender = address!("0x000000000000000000000000000000000000000a");
        let b_sender = address!("0x000000000000000000000000000000000000000b");
        let c_sender = address!("0x000000000000000000000000000000000000000c");
        let d_sender = address!("0x000000000000000000000000000000000000000d");

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
        let a_sender = address!("0x000000000000000000000000000000000000000a");

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

        let a_sender = address!("0x000000000000000000000000000000000000000a");
        let b_sender = address!("0x000000000000000000000000000000000000000b");
        let c_sender = address!("0x000000000000000000000000000000000000000c");
        let d_sender = address!("0x000000000000000000000000000000000000000d");

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

        // add all the transactions to the pool
        for tx in all_txs {
            pool.add_transaction(f.validated_arc(tx));
        }

        // get senders by submission id - a4, b3, c3, d1, reversed
        let senders =
            pool.get_senders_by_submission_id().map(|s| s.sender_id()).collect::<Vec<_>>();
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

        let senders =
            pool.get_senders_by_submission_id().map(|s| s.sender_id()).collect::<Vec<_>>();
        assert_eq!(senders.len(), 4);
        let expected_senders = vec![a_sender, c_sender, b_sender, d_sender]
            .into_iter()
            .map(|s| f.ids.sender_id(&s).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(senders, expected_senders);
    }

    #[test]
    fn test_add_sender_count_new_sender() {
        // Initialize a mock transaction factory
        let mut f = MockTransactionFactory::default();
        // Create an empty transaction pool
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        // Generate a validated transaction and add it to the pool
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx);

        // Define a new sender ID and submission ID
        let sender: SenderId = 11.into();
        let submission_id = 1;

        // Add the sender count to the pool
        pool.add_sender_count(sender, submission_id);

        // Assert that the sender transaction count is updated correctly
        assert_eq!(pool.sender_id_count.len(), 2);
        let sender_info = pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender).unwrap();
        assert_eq!(sender_info.count(), 1);
        // Find the corresponding submission info for this sender
        let submission_info =
            pool.sender_id_last_submission.iter().find(|ss| ss.sender_id() == sender).unwrap();
        assert_eq!(submission_info.sender_id(), sender);
        assert_eq!(submission_info.submission_id(), submission_id);

        // Assert that the last sender submission is updated correctly
        assert_eq!(pool.sender_id_last_submission.len(), 2);
    }

    #[test]
    fn test_add_sender_count_existing_sender() {
        // Initialize a mock transaction factory
        let mut f = MockTransactionFactory::default();
        // Create an empty transaction pool
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        // Generate a validated transaction and add it to the pool
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx);

        // Define a sender ID and initial submission ID
        let sender: SenderId = 11.into();
        let initial_submission_id = 1;

        // Add the sender count to the pool with the initial submission ID
        pool.add_sender_count(sender, initial_submission_id);

        // Define a new submission ID
        let new_submission_id = 2;
        // Add the sender count to the pool with the new submission ID
        pool.add_sender_count(sender, new_submission_id);

        // Assert that the sender transaction count is updated correctly
        assert_eq!(pool.sender_id_count.len(), 2);
        let sender_info = pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender).unwrap();
        assert_eq!(sender_info.count(), 2);

        // Assert that the last sender submission is updated correctly
        assert_eq!(pool.sender_id_last_submission.len(), 2);
        let submission_info =
            pool.sender_id_last_submission.iter().find(|ss| ss.sender_id() == sender).unwrap();
        assert_eq!(submission_info.sender_id(), sender);
        assert_eq!(submission_info.submission_id(), new_submission_id);
    }

    #[test]
    fn test_add_sender_count_multiple_senders() {
        // Initialize a mock transaction factory
        let mut f = MockTransactionFactory::default();
        // Create an empty transaction pool
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        // Generate two validated transactions and add them to the pool
        let tx1 = f.validated_arc(MockTransaction::eip1559().inc_price());
        let tx2 = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx1);
        pool.add_transaction(tx2);

        // Define two different sender IDs and their corresponding submission IDs
        let sender1: SenderId = 11.into();
        let sender2: SenderId = 22.into();

        // Add the sender counts to the pool
        pool.add_sender_count(sender1, 1);
        pool.add_sender_count(sender2, 2);

        // Assert that the sender transaction counts are updated correctly
        assert_eq!(pool.sender_id_count.len(), 4);

        let sender1_info =
            pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender1).unwrap();
        assert_eq!(sender1_info.count(), 1);

        let sender2_info =
            pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender2).unwrap();
        assert_eq!(sender2_info.count(), 1);

        // Assert that the last sender submission is updated correctly
        assert_eq!(pool.sender_id_last_submission.len(), 4);

        // Verify that sender 1 is in the last sender submission
        let submission_info1 =
            pool.sender_id_last_submission.iter().find(|info| info.sender_id() == sender1);
        assert!(submission_info1.is_some());
        assert_eq!(submission_info1.unwrap().submission_id(), 1);

        // Verify that sender 2 is in the last sender submission
        let submission_info2 =
            pool.sender_id_last_submission.iter().find(|info| info.sender_id() == sender2);
        assert!(submission_info2.is_some());
        assert_eq!(submission_info2.unwrap().submission_id(), 2);
    }

    #[test]
    fn test_remove_sender_count() {
        // Initialize a mock transaction factory
        let mut f = MockTransactionFactory::default();
        // Create an empty transaction pool
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        // Generate two validated transactions and add them to the pool
        let tx1 = f.validated_arc(MockTransaction::eip1559().inc_price());
        let tx2 = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx1);
        pool.add_transaction(tx2);

        // Define two different sender IDs and their corresponding submission IDs
        let sender1: SenderId = 11.into();
        let sender2: SenderId = 22.into();

        // Add the sender counts to the pool
        pool.add_sender_count(sender1, 1);

        // We add sender 2 multiple times to test the removal of sender counts
        pool.add_sender_count(sender2, 2);
        pool.add_sender_count(sender2, 3);

        // Before removing the sender count we should have 4 sender transaction counts
        assert_eq!(pool.sender_id_count.len(), 4);
        assert!(pool.sender_id_count.iter().any(|sc| sc.sender_id() == sender1));

        // We should have 1 sender transaction count for sender 1 before removing the sender
        let sender1_count =
            pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender1).unwrap();
        assert_eq!(sender1_count.count(), 1);

        // Remove the sender count for sender 1
        pool.remove_sender_count(sender1);

        // After removing the sender count we should have 3 sender transaction counts remaining
        assert_eq!(pool.sender_id_count.len(), 3);
        assert!(!pool.sender_id_count.iter().any(|sc| sc.sender_id() == sender1));

        // Check the sender transaction count for sender 2 before removing the sender count
        let sender2_count =
            pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender2).unwrap();
        assert_eq!(sender2_count.count(), 2);

        // The last submission id for sender2 should be 3
        let sender2_last_submission =
            pool.sender_id_last_submission.iter().find(|ss| ss.sender_id() == sender2).unwrap();
        assert_eq!(sender2_last_submission.submission_id(), 3);

        // Remove the sender count for sender 2
        pool.remove_sender_count(sender2);

        // After removing the sender count for sender 2, we still have 3 sender transaction counts
        // remaining, because sender2's count is now 1 (not removed yet).
        assert_eq!(pool.sender_id_count.len(), 3);
        assert!(pool.sender_id_count.iter().any(|sc| sc.sender_id() == sender2));

        // Sender transaction count for sender 2 should be updated correctly
        let sender2_count =
            pool.sender_id_count.iter().find(|sc| sc.sender_id() == sender2).unwrap();
        assert_eq!(sender2_count.count(), 1);

        // The last submission id for sender2 should still be 3
        let sender2_last_submission =
            pool.sender_id_last_submission.iter().find(|ss| ss.sender_id() == sender2).unwrap();
        assert_eq!(sender2_last_submission.submission_id(), 3);
    }

    #[test]
    fn test_pool_size() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Create a transaction with a specific size and add it to the pool
        let tx = f.validated_arc(MockTransaction::eip1559().set_size(1024).clone());
        pool.add_transaction(tx);

        // Assert that the reported size of the pool is correct
        assert_eq!(pool.size(), 1024);
    }

    #[test]
    fn test_pool_len() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Initially, the pool should have zero transactions
        assert_eq!(pool.len(), 0);

        // Add a transaction to the pool and check the length
        let tx = f.validated_arc(MockTransaction::eip1559());
        pool.add_transaction(tx);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_pool_contains() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Create a transaction and get its ID
        let tx = f.validated_arc(MockTransaction::eip1559());
        let tx_id = *tx.id();

        // Before adding, the transaction should not be in the pool
        assert!(!pool.contains(&tx_id));

        // After adding, the transaction should be present in the pool
        pool.add_transaction(tx);
        assert!(pool.contains(&tx_id));
    }

    #[test]
    fn test_get_transaction() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Add a transaction to the pool and get its ID
        let tx = f.validated_arc(MockTransaction::eip1559());
        let tx_id = *tx.id();
        pool.add_transaction(tx.clone());

        // Retrieve the transaction using `get()` and assert it matches the added transaction
        let retrieved = pool.get(&tx_id).expect("Transaction should exist in the pool");
        assert_eq!(retrieved.transaction.id(), tx.id());
    }

    #[test]
    fn test_all_transactions() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Add two transactions to the pool
        let tx1 = f.validated_arc(MockTransaction::eip1559());
        let tx2 = f.validated_arc(MockTransaction::eip1559());
        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        // Collect all transaction IDs from the pool
        let all_txs: Vec<_> = pool.all().map(|tx| *tx.id()).collect();
        assert_eq!(all_txs.len(), 2);

        // Check that the IDs of both transactions are present
        assert!(all_txs.contains(tx1.id()));
        assert!(all_txs.contains(tx2.id()));
    }

    #[test]
    fn test_truncate_pool_edge_case() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Add two transactions to the pool
        let tx1 = f.validated_arc(MockTransaction::eip1559());
        let tx2 = f.validated_arc(MockTransaction::eip1559());
        pool.add_transaction(tx1);
        pool.add_transaction(tx2);

        // Set a limit that matches the current number of transactions
        let limit = SubPoolLimit { max_txs: 2, max_size: usize::MAX };
        let removed = pool.truncate_pool(limit);

        // No transactions should be removed
        assert!(removed.is_empty());

        // Set a stricter limit that requires truncating one transaction
        let limit = SubPoolLimit { max_txs: 1, max_size: usize::MAX };
        let removed = pool.truncate_pool(limit);

        // One transaction should be removed, and the pool should have one left
        assert_eq!(removed.len(), 1);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_satisfy_base_fee_transactions() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Add two transactions with different max fees
        let tx1 = f.validated_arc(MockTransaction::eip1559().set_max_fee(100).clone());
        let tx2 = f.validated_arc(MockTransaction::eip1559().set_max_fee(200).clone());
        pool.add_transaction(tx1);
        pool.add_transaction(tx2.clone());

        // Check that only the second transaction satisfies the base fee requirement
        let satisfied = pool.satisfy_base_fee_transactions(150);
        assert_eq!(satisfied.len(), 1);
        assert_eq!(satisfied[0].id(), tx2.id())
    }

    #[test]
    fn test_remove_transaction() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();

        // Add a transaction to the pool and get its ID
        let tx = f.validated_arc(MockTransaction::eip1559());
        let tx_id = *tx.id();
        pool.add_transaction(tx);

        // Ensure the transaction is in the pool before removal
        assert!(pool.contains(&tx_id));

        // Remove the transaction and check that it is no longer in the pool
        let removed = pool.remove_transaction(&tx_id);
        assert!(removed.is_some());
        assert!(!pool.contains(&tx_id));
    }
}
