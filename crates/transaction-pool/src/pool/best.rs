use crate::{
    identifier::{SenderId, TransactionId},
    pool::pending::PendingTransaction,
    PayloadTransactions, PoolTransaction, TransactionOrdering, ValidPoolTransaction,
};
use alloy_consensus::Transaction;
use alloy_primitives::Address;
use core::fmt;
use reth_primitives::TransactionSignedEcRecovered;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::Arc,
};
use tokio::sync::broadcast::{error::TryRecvError, Receiver};
use tracing::debug;

/// An iterator that returns transactions that can be executed on the current state (*best*
/// transactions).
///
/// This is a wrapper around [`BestTransactions`] that also enforces a specific basefee.
///
/// This iterator guarantees that all transaction it returns satisfy both the base fee and blob fee!
#[derive(Debug)]
pub(crate) struct BestTransactionsWithFees<T: TransactionOrdering> {
    pub(crate) best: BestTransactions<T>,
    pub(crate) base_fee: u64,
    pub(crate) base_fee_per_blob_gas: u64,
}

impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactionsWithFees<T> {
    fn mark_invalid(&mut self, tx: &Self::Item) {
        BestTransactions::mark_invalid(&mut self.best, tx)
    }

    fn no_updates(&mut self) {
        self.best.no_updates()
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true)
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.best.set_skip_blobs(skip_blobs)
    }
}

impl<T: TransactionOrdering> Iterator for BestTransactionsWithFees<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        // find the next transaction that satisfies the base fee
        loop {
            let best = Iterator::next(&mut self.best)?;
            // If both the base fee and blob fee (if applicable for EIP-4844) are satisfied, return
            // the transaction
            if best.transaction.max_fee_per_gas() >= self.base_fee as u128 &&
                best.transaction
                    .max_fee_per_blob_gas()
                    .map_or(true, |fee| fee >= self.base_fee_per_blob_gas as u128)
            {
                return Some(best);
            }
            crate::traits::BestTransactions::mark_invalid(self, &best);
        }
    }
}

/// An iterator that returns transactions that can be executed on the current state (*best*
/// transactions).
///
/// The [`PendingPool`](crate::pool::pending::PendingPool) contains transactions that *could* all
/// be executed on the current state, but only yields transactions that are ready to be executed
/// now. While it contains all gapless transactions of a sender, it _always_ only returns the
/// transaction with the current on chain nonce.
#[derive(Debug)]
pub(crate) struct BestTransactions<T: TransactionOrdering> {
    /// Contains a copy of _all_ transactions of the pending pool at the point in time this
    /// iterator was created.
    pub(crate) all: BTreeMap<TransactionId, PendingTransaction<T>>,
    /// Transactions that can be executed right away: these have the expected nonce.
    ///
    /// Once an `independent` transaction with the nonce `N` is returned, it unlocks `N+1`, which
    /// then can be moved from the `all` set to the `independent` set.
    pub(crate) independent: BTreeSet<PendingTransaction<T>>,
    /// There might be the case where a yielded transactions is invalid, this will track it.
    pub(crate) invalid: HashSet<SenderId>,
    /// Used to receive any new pending transactions that have been added to the pool after this
    /// iterator was static fileted
    ///
    /// These new pending transactions are inserted into this iterator's pool before yielding the
    /// next value
    pub(crate) new_transaction_receiver: Option<Receiver<PendingTransaction<T>>>,
    /// Flag to control whether to skip blob transactions (EIP4844).
    pub(crate) skip_blobs: bool,
}

impl<T: TransactionOrdering> BestTransactions<T> {
    /// Mark the transaction and it's descendants as invalid.
    pub(crate) fn mark_invalid(&mut self, tx: &Arc<ValidPoolTransaction<T::Transaction>>) {
        self.invalid.insert(tx.sender_id());
    }

    /// Returns the ancestor the given transaction, the transaction with `nonce - 1`.
    ///
    /// Note: for a transaction with nonce higher than the current on chain nonce this will always
    /// return an ancestor since all transaction in this pool are gapless.
    pub(crate) fn ancestor(&self, id: &TransactionId) -> Option<&PendingTransaction<T>> {
        self.all.get(&id.unchecked_ancestor()?)
    }

    /// Non-blocking read on the new pending transactions subscription channel
    fn try_recv(&mut self) -> Option<PendingTransaction<T>> {
        loop {
            match self.new_transaction_receiver.as_mut()?.try_recv() {
                Ok(tx) => return Some(tx),
                // note TryRecvError::Lagged can be returned here, which is an error that attempts
                // to correct itself on consecutive try_recv() attempts

                // the cost of ignoring this error is allowing old transactions to get
                // overwritten after the chan buffer size is met
                Err(TryRecvError::Lagged(_)) => {
                    // Handle the case where the receiver lagged too far behind.
                    // `num_skipped` indicates the number of messages that were skipped.
                    continue
                }

                // this case is still better than the existing iterator behavior where no new
                // pending txs are surfaced to consumers
                Err(_) => return None,
            }
        }
    }

    /// Removes the currently best independent transaction from the independent set and the total
    /// set.
    fn pop_best(&mut self) -> Option<PendingTransaction<T>> {
        self.independent.pop_last().inspect(|best| {
            let removed = self.all.remove(best.transaction.id());
            debug_assert!(removed.is_some(), "must be present in both sets");
        })
    }

    /// Checks for new transactions that have come into the `PendingPool` after this iterator was
    /// created and inserts them
    fn add_new_transactions(&mut self) {
        while let Some(pending_tx) = self.try_recv() {
            //  same logic as PendingPool::add_transaction/PendingPool::best_with_unlocked
            let tx_id = *pending_tx.transaction.id();
            if self.ancestor(&tx_id).is_none() {
                self.independent.insert(pending_tx.clone());
            }
            self.all.insert(tx_id, pending_tx);
        }
    }
}

impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item) {
        Self::mark_invalid(self, tx)
    }

    fn no_updates(&mut self) {
        self.new_transaction_receiver.take();
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true);
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.skip_blobs = skip_blobs;
    }
}

impl<T: TransactionOrdering> Iterator for BestTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.add_new_transactions();
            // Remove the next independent tx with the highest priority
            let best = self.pop_best()?;
            let sender_id = best.transaction.sender_id();

            // skip transactions for which sender was marked as invalid
            if self.invalid.contains(&sender_id) {
                debug!(
                    target: "txpool",
                    "[{:?}] skipping invalid transaction",
                    best.transaction.hash()
                );
                continue
            }

            // Insert transactions that just got unlocked.
            if let Some(unlocked) = self.all.get(&best.unlocks()) {
                self.independent.insert(unlocked.clone());
            }

            if self.skip_blobs && best.transaction.transaction.is_eip4844() {
                // blobs should be skipped, marking them as invalid will ensure that no dependent
                // transactions are returned
                self.mark_invalid(&best.transaction)
            } else {
                return Some(best.transaction)
            }
        }
    }
}

/// Wrapper struct that allows to convert `BestTransactions` (used in tx pool) to
/// `PayloadTransactions` (used in block composition).
#[derive(Debug)]
pub struct BestPayloadTransactions<T, I>
where
    T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    invalid: HashSet<Address>,
    best: I,
}

impl<T, I> BestPayloadTransactions<T, I>
where
    T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    /// Create a new `BestPayloadTransactions` with the given iterator.
    pub fn new(best: I) -> Self {
        Self { invalid: Default::default(), best }
    }
}

impl<T, I> PayloadTransactions for BestPayloadTransactions<T, I>
where
    T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    fn next(&mut self, _ctx: ()) -> Option<TransactionSignedEcRecovered> {
        loop {
            let tx = self.best.next()?;
            if self.invalid.contains(&tx.sender()) {
                continue
            }
            return Some(tx.to_recovered_transaction())
        }
    }

    fn mark_invalid(&mut self, sender: Address, _nonce: u64) {
        self.invalid.insert(sender);
    }
}

/// A [`BestTransactions`](crate::traits::BestTransactions) implementation that filters the
/// transactions of iter with predicate.
///
/// Filter out transactions are marked as invalid:
/// [`BestTransactions::mark_invalid`](crate::traits::BestTransactions::mark_invalid).
pub struct BestTransactionFilter<I, P> {
    pub(crate) best: I,
    pub(crate) predicate: P,
}

impl<I, P> BestTransactionFilter<I, P> {
    /// Create a new [`BestTransactionFilter`] with the given predicate.
    pub const fn new(best: I, predicate: P) -> Self {
        Self { best, predicate }
    }
}

impl<I, P> Iterator for BestTransactionFilter<I, P>
where
    I: crate::traits::BestTransactions,
    P: FnMut(&<I as Iterator>::Item) -> bool,
{
    type Item = <I as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let best = self.best.next()?;
            if (self.predicate)(&best) {
                return Some(best)
            }
            self.best.mark_invalid(&best);
        }
    }
}

impl<I, P> crate::traits::BestTransactions for BestTransactionFilter<I, P>
where
    I: crate::traits::BestTransactions,
    P: FnMut(&<I as Iterator>::Item) -> bool + Send,
{
    fn mark_invalid(&mut self, tx: &Self::Item) {
        crate::traits::BestTransactions::mark_invalid(&mut self.best, tx)
    }

    fn no_updates(&mut self) {
        self.best.no_updates()
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true)
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.best.set_skip_blobs(skip_blobs)
    }
}

impl<I: fmt::Debug, P> fmt::Debug for BestTransactionFilter<I, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BestTransactionFilter").field("best", &self.best).finish()
    }
}

/// Wrapper over [`crate::traits::BestTransactions`] that prioritizes transactions of certain
/// senders capping total gas used by such transactions.
#[derive(Debug)]
pub struct BestTransactionsWithPrioritizedSenders<I: Iterator> {
    /// Inner iterator
    inner: I,
    /// A set of senders which transactions should be prioritized
    prioritized_senders: HashSet<Address>,
    /// Maximum total gas limit of prioritized transactions
    max_prioritized_gas: u64,
    /// Buffer with transactions that are not being prioritized. Those will be the first to be
    /// included after the prioritized transactions
    buffer: VecDeque<I::Item>,
    /// Tracker of total gas limit of prioritized transactions. Once it reaches
    /// `max_prioritized_gas` no more transactions will be prioritized
    prioritized_gas: u64,
}

impl<I: Iterator> BestTransactionsWithPrioritizedSenders<I> {
    /// Constructs a new [`BestTransactionsWithPrioritizedSenders`].
    pub fn new(prioritized_senders: HashSet<Address>, max_prioritized_gas: u64, inner: I) -> Self {
        Self {
            inner,
            prioritized_senders,
            max_prioritized_gas,
            buffer: Default::default(),
            prioritized_gas: Default::default(),
        }
    }
}

impl<I, T> Iterator for BestTransactionsWithPrioritizedSenders<I>
where
    I: crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction,
{
    type Item = <I as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        // If we have space, try prioritizing transactions
        if self.prioritized_gas < self.max_prioritized_gas {
            for item in &mut self.inner {
                if self.prioritized_senders.contains(&item.transaction.sender()) &&
                    self.prioritized_gas + item.transaction.gas_limit() <=
                        self.max_prioritized_gas
                {
                    self.prioritized_gas += item.transaction.gas_limit();
                    return Some(item)
                }
                self.buffer.push_back(item);
            }
        }

        if let Some(item) = self.buffer.pop_front() {
            Some(item)
        } else {
            self.inner.next()
        }
    }
}

impl<I, T> crate::traits::BestTransactions for BestTransactionsWithPrioritizedSenders<I>
where
    I: crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction,
{
    fn mark_invalid(&mut self, tx: &Self::Item) {
        self.inner.mark_invalid(tx)
    }

    fn no_updates(&mut self) {
        self.inner.no_updates()
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        if skip_blobs {
            self.buffer.retain(|tx| !tx.transaction.is_eip4844())
        }
        self.inner.set_skip_blobs(skip_blobs)
    }
}

/// An implementation of [`crate::traits::PayloadTransactions`] that yields
/// a pre-defined set of transactions.
///
/// This is useful to put a sequencer-specified set of transactions into the block
/// and compose it with the rest of the transactions.
#[derive(Debug)]
pub struct PayloadTransactionsFixed<T> {
    transactions: Vec<T>,
    index: usize,
}

impl<T> PayloadTransactionsFixed<T> {
    /// Constructs a new [`PayloadTransactionsFixed`].
    pub fn new(transactions: Vec<T>) -> Self {
        Self { transactions, index: Default::default() }
    }

    /// Constructs a new [`PayloadTransactionsFixed`] with a single transaction.
    pub fn single(transaction: T) -> Self {
        Self { transactions: vec![transaction], index: Default::default() }
    }
}

impl PayloadTransactions for PayloadTransactionsFixed<TransactionSignedEcRecovered> {
    fn next(&mut self, _ctx: ()) -> Option<TransactionSignedEcRecovered> {
        (self.index < self.transactions.len()).then(|| {
            let tx = self.transactions[self.index].clone();
            self.index += 1;
            tx
        })
    }

    fn mark_invalid(&mut self, _sender: Address, _nonce: u64) {}
}

/// Wrapper over [`crate::traits::PayloadTransactions`] that combines transactions from multiple
/// `PayloadTransactions` iterators and keeps track of the gas for both of iterators.
///
/// We can't use [`Iterator::chain`], because:
/// (a) we need to propagate the `mark_invalid` and `no_updates`
/// (b) we need to keep track of the gas
///
/// Notes that [`PayloadTransactionsChain`] fully drains the first iterator
/// before moving to the second one.
///
/// If the `before` iterator has transactions that are not fitting into the block,
/// the after iterator will get propagated a `mark_invalid` call for each of them.
#[derive(Debug)]
pub struct PayloadTransactionsChain<B: PayloadTransactions, A: PayloadTransactions> {
    /// Iterator that will be used first
    before: B,
    /// Allowed gas for the transactions from `before` iterator. If `None`, no gas limit is
    /// enforced.
    before_max_gas: Option<u64>,
    /// Gas used by the transactions from `before` iterator
    before_gas: u64,
    /// Iterator that will be used after `before` iterator
    after: A,
    /// Allowed gas for the transactions from `after` iterator. If `None`, no gas limit is
    /// enforced.
    after_max_gas: Option<u64>,
    /// Gas used by the transactions from `after` iterator
    after_gas: u64,
}

impl<B: PayloadTransactions, A: PayloadTransactions> PayloadTransactionsChain<B, A> {
    /// Constructs a new [`PayloadTransactionsChain`].
    pub fn new(
        before: B,
        before_max_gas: Option<u64>,
        after: A,
        after_max_gas: Option<u64>,
    ) -> Self {
        Self {
            before,
            before_max_gas,
            before_gas: Default::default(),
            after,
            after_max_gas,
            after_gas: Default::default(),
        }
    }
}

impl<B, A> PayloadTransactions for PayloadTransactionsChain<B, A>
where
    B: PayloadTransactions,
    A: PayloadTransactions,
{
    fn next(&mut self, ctx: ()) -> Option<TransactionSignedEcRecovered> {
        while let Some(tx) = self.before.next(ctx) {
            if let Some(before_max_gas) = self.before_max_gas {
                if self.before_gas + tx.transaction.gas_limit() <= before_max_gas {
                    self.before_gas += tx.transaction.gas_limit();
                    return Some(tx);
                }
                self.before.mark_invalid(tx.signer(), tx.transaction.nonce());
                self.after.mark_invalid(tx.signer(), tx.transaction.nonce());
            } else {
                return Some(tx);
            }
        }

        while let Some(tx) = self.after.next(ctx) {
            if let Some(after_max_gas) = self.after_max_gas {
                if self.after_gas + tx.transaction.gas_limit() <= after_max_gas {
                    self.after_gas += tx.transaction.gas_limit();
                    return Some(tx);
                }
                self.after.mark_invalid(tx.signer(), tx.transaction.nonce());
            } else {
                return Some(tx);
            }
        }

        None
    }

    fn mark_invalid(&mut self, sender: Address, nonce: u64) {
        self.before.mark_invalid(sender, nonce);
        self.after.mark_invalid(sender, nonce);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pool::pending::PendingPool,
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
        Priority,
    };
    use alloy_primitives::U256;

    #[test]
    fn test_best_iter() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best();
        assert_eq!(best.all.len(), num_tx as usize);
        assert_eq!(best.independent.len(), 1);

        // check tx are returned in order
        for nonce in 0..num_tx {
            assert_eq!(best.independent.len(), 1);
            let tx = best.next().unwrap();
            assert_eq!(tx.nonce(), nonce);
        }
    }

    #[test]
    fn test_best_iter_invalid() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best();

        // mark the first tx as invalid
        let invalid = best.independent.iter().next().unwrap();
        best.mark_invalid(&invalid.transaction.clone());

        // iterator is empty
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_transactions_iter_invalid() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best: Box<
            dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<MockTransaction>>>,
        > = Box::new(pool.best());

        let tx = Iterator::next(&mut best).unwrap();
        crate::traits::BestTransactions::mark_invalid(&mut *best, &tx);
        assert!(Iterator::next(&mut best).is_none());
    }

    #[test]
    fn test_best_with_fees_iter_base_fee_satisfied() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 15;

        // Insert transactions with a max_fee_per_gas greater than or equal to the base fee
        // Without blob fee
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip1559()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        for nonce in 0..num_tx {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.nonce(), nonce);
            assert!(tx.transaction.max_fee_per_gas() >= base_fee as u128);
        }
    }

    #[test]
    fn test_best_with_fees_iter_base_fee_violated() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 20;
        let base_fee_per_blob_gas: u64 = 15;

        // Insert transactions with a max_fee_per_gas less than the base fee
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip1559()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 - 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // No transaction should be returned since all violate the base fee
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_blob_fee_satisfied() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with a max_fee_per_blob_gas greater than or equal to the base fee per
        // blob gas
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip4844()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5)
                .with_blob_fee(base_fee_per_blob_gas as u128 + 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // All transactions should be returned in order since they satisfy both base fee and blob
        // fee
        for nonce in 0..num_tx {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.nonce(), nonce);
            assert!(tx.transaction.max_fee_per_gas() >= base_fee as u128);
            assert!(
                tx.transaction.max_fee_per_blob_gas().unwrap() >= base_fee_per_blob_gas as u128
            );
        }

        // No more transactions should be returned
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_blob_fee_violated() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with a max_fee_per_blob_gas less than the base fee per blob gas
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip4844()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5)
                .with_blob_fee(base_fee_per_blob_gas as u128 - 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // No transaction should be returned since all violate the blob fee
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_mixed_fees() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with varying max_fee_per_gas and max_fee_per_blob_gas
        let tx1 =
            MockTransaction::eip1559().rng_hash().with_nonce(0).with_max_fee(base_fee as u128 + 5);
        let tx2 = MockTransaction::eip4844()
            .rng_hash()
            .with_nonce(1)
            .with_max_fee(base_fee as u128 + 5)
            .with_blob_fee(base_fee_per_blob_gas as u128 + 5);
        let tx3 = MockTransaction::eip4844()
            .rng_hash()
            .with_nonce(2)
            .with_max_fee(base_fee as u128 + 5)
            .with_blob_fee(base_fee_per_blob_gas as u128 - 5);
        let tx4 =
            MockTransaction::eip1559().rng_hash().with_nonce(3).with_max_fee(base_fee as u128 - 5);

        pool.add_transaction(Arc::new(f.validated(tx1.clone())), 0);
        pool.add_transaction(Arc::new(f.validated(tx2.clone())), 0);
        pool.add_transaction(Arc::new(f.validated(tx3)), 0);
        pool.add_transaction(Arc::new(f.validated(tx4)), 0);

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        let expected_order = vec![tx1, tx2];
        for expected_tx in expected_order {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.transaction, expected_tx);
        }

        // No more transactions should be returned
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_add_transaction_with_next_nonce() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a BestTransactions iterator from the pool
        let mut best = pool.best();

        // Use a broadcast channel for transaction updates
        let (tx_sender, tx_receiver) =
            tokio::sync::broadcast::channel::<PendingTransaction<MockOrdering>>(1000);
        best.new_transaction_receiver = Some(tx_receiver);

        // Create a new transaction with nonce 5 and validate it
        let new_tx = MockTransaction::eip1559().rng_hash().with_nonce(5);
        let valid_new_tx = f.validated(new_tx);

        // Send the new transaction through the broadcast channel
        let pending_tx = PendingTransaction {
            submission_id: 10,
            transaction: Arc::new(valid_new_tx.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx.clone()).unwrap();

        // Add new transactions to the iterator
        best.add_new_transactions();

        // Verify that the new transaction has been added to the 'all' map
        assert_eq!(best.all.len(), 6);
        assert!(best.all.contains_key(valid_new_tx.id()));

        // Verify that the new transaction has been added to the 'independent' set
        assert_eq!(best.independent.len(), 2);
        assert!(best.independent.contains(&pending_tx));
    }

    #[test]
    fn test_best_add_transaction_with_ancestor() {
        // Initialize a new PendingPool with default MockOrdering and MockTransactionFactory
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a BestTransactions iterator from the pool
        let mut best = pool.best();

        // Use a broadcast channel for transaction updates
        let (tx_sender, tx_receiver) =
            tokio::sync::broadcast::channel::<PendingTransaction<MockOrdering>>(1000);
        best.new_transaction_receiver = Some(tx_receiver);

        // Create a new transaction with nonce 5 and validate it
        let base_tx1 = MockTransaction::eip1559().rng_hash().with_nonce(5);
        let valid_new_tx1 = f.validated(base_tx1.clone());

        // Send the new transaction through the broadcast channel
        let pending_tx1 = PendingTransaction {
            submission_id: 10,
            transaction: Arc::new(valid_new_tx1.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx1.clone()).unwrap();

        // Add new transactions to the iterator
        best.add_new_transactions();

        // Verify that the new transaction has been added to the 'all' map
        assert_eq!(best.all.len(), 6);
        assert!(best.all.contains_key(valid_new_tx1.id()));

        // Verify that the new transaction has been added to the 'independent' set
        assert_eq!(best.independent.len(), 2);
        assert!(best.independent.contains(&pending_tx1));

        // Attempt to add a new transaction with a different nonce (not a duplicate)
        let base_tx2 = base_tx1.with_nonce(6);
        let valid_new_tx2 = f.validated(base_tx2);

        // Send the new transaction through the broadcast channel
        let pending_tx2 = PendingTransaction {
            submission_id: 11, // Different submission ID
            transaction: Arc::new(valid_new_tx2.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx2.clone()).unwrap();

        // Add new transactions to the iterator
        best.add_new_transactions();

        // Verify that the new transaction has been added to 'all'
        assert_eq!(best.all.len(), 7);
        assert!(best.all.contains_key(valid_new_tx2.id()));

        // Verify that the new transaction has not been added to the 'independent' set
        assert_eq!(best.independent.len(), 2);
        assert!(!best.independent.contains(&pending_tx2));
    }

    #[test]
    fn test_best_transactions_filter_trait_object() {
        // Initialize a new PendingPool with default MockOrdering and MockTransactionFactory
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a trait object of BestTransactions iterator from the pool
        let best: Box<dyn crate::traits::BestTransactions<Item = _>> = Box::new(pool.best());

        // Create a filter that only returns transactions with even nonces
        let filter =
            BestTransactionFilter::new(best, |tx: &Arc<ValidPoolTransaction<MockTransaction>>| {
                tx.nonce() % 2 == 0
            });

        // Verify that the filter only returns transactions with even nonces
        for tx in filter {
            assert_eq!(tx.nonce() % 2, 0);
        }
    }

    #[test]
    fn test_best_transactions_prioritized_senders() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 plain transactions from different senders with increasing gas price
        for gas_price in 0..5 {
            let tx = MockTransaction::eip1559().with_gas_price(gas_price);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Add another transaction with 0 gas price that's going to be prioritized by sender
        let prioritized_tx = MockTransaction::eip1559().with_gas_price(0);
        let valid_prioritized_tx = f.validated(prioritized_tx.clone());
        pool.add_transaction(Arc::new(valid_prioritized_tx), 0);

        let prioritized_senders = HashSet::from([prioritized_tx.sender()]);
        let best =
            BestTransactionsWithPrioritizedSenders::new(prioritized_senders, 200, pool.best());

        // Verify that the prioritized transaction is returned first
        // and the rest are returned in the reverse order of gas price
        let mut iter = best.into_iter();
        let top_of_block_tx = iter.next().unwrap();
        assert_eq!(top_of_block_tx.max_fee_per_gas(), 0);
        assert_eq!(top_of_block_tx.sender(), prioritized_tx.sender());
        for gas_price in (0..5).rev() {
            assert_eq!(iter.next().unwrap().max_fee_per_gas(), gas_price);
        }

        // TODO: Test that gas limits for prioritized transactions are respected
    }

    #[test]
    fn test_best_transactions_chained_iterators() {
        let mut priority_pool = PendingPool::new(MockOrdering::default());
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Block composition
        // ===
        // (1) up to 100 gas: custom top-of-block transaction
        // (2) up to 100 gas: transactions from the priority pool
        // (3) up to 200 gas: only transactions from address A
        // (4) up to 200 gas: only transactions from address B
        // (5) until block gas limit: all transactions from the main pool

        // Notes:
        // - If prioritized addresses overlap, a single transaction will be prioritized twice and
        //   therefore use the per-segment gas limit twice.
        // - Priority pool and main pool must synchronize between each other to make sure there are
        //   no conflicts for the same nonce. For example, in this scenario, pools can't reject
        //   transactions with seemingly incorrect nonces, because previous transactions might be in
        //   the other pool.

        let address_top_of_block = Address::random();
        let address_in_priority_pool = Address::random();
        let address_a = Address::random();
        let address_b = Address::random();
        let address_regular = Address::random();

        // Add transactions to the main pool
        {
            let prioritized_tx_a =
                MockTransaction::eip1559().with_gas_price(5).with_sender(address_a);
            // without our custom logic, B would be prioritized over A due to gas price:
            let prioritized_tx_b =
                MockTransaction::eip1559().with_gas_price(10).with_sender(address_b);
            let regular_tx =
                MockTransaction::eip1559().with_gas_price(15).with_sender(address_regular);
            pool.add_transaction(Arc::new(f.validated(prioritized_tx_a)), 0);
            pool.add_transaction(Arc::new(f.validated(prioritized_tx_b)), 0);
            pool.add_transaction(Arc::new(f.validated(regular_tx)), 0);
        }

        // Add transactions to the priority pool
        {
            let prioritized_tx =
                MockTransaction::eip1559().with_gas_price(0).with_sender(address_in_priority_pool);
            let valid_prioritized_tx = f.validated(prioritized_tx);
            priority_pool.add_transaction(Arc::new(valid_prioritized_tx), 0);
        }

        let mut block = PayloadTransactionsChain::new(
            PayloadTransactionsFixed::single(
                MockTransaction::eip1559().with_sender(address_top_of_block).into(),
            ),
            Some(100),
            PayloadTransactionsChain::new(
                BestPayloadTransactions::new(priority_pool.best()),
                Some(100),
                BestPayloadTransactions::new(BestTransactionsWithPrioritizedSenders::new(
                    HashSet::from([address_a]),
                    200,
                    BestTransactionsWithPrioritizedSenders::new(
                        HashSet::from([address_b]),
                        200,
                        pool.best(),
                    ),
                )),
                None,
            ),
            None,
        );

        assert_eq!(block.next(()).unwrap().signer(), address_top_of_block);
        assert_eq!(block.next(()).unwrap().signer(), address_in_priority_pool);
        assert_eq!(block.next(()).unwrap().signer(), address_a);
        assert_eq!(block.next(()).unwrap().signer(), address_b);
        assert_eq!(block.next(()).unwrap().signer(), address_regular);
    }

    // TODO: Same nonce test
}
