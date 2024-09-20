use crate::{
    identifier::TransactionId, pool::pending::PendingTransaction, PoolTransaction,
    TransactionOrdering, ValidPoolTransaction,
};
use alloy_primitives::B256 as TxHash;
use core::fmt;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
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
            let best = self.best.next()?;
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
    pub(crate) invalid: HashSet<TxHash>,
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
        self.invalid.insert(*tx.hash());
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

    /// Checks for new transactions that have come into the `PendingPool` after this iterator was
    /// created and inserts them
    fn add_new_transactions(&mut self) {
        while let Some(pending_tx) = self.try_recv() {
            let tx = pending_tx.transaction.clone();
            //  same logic as PendingPool::add_transaction/PendingPool::best_with_unlocked
            let tx_id = *tx.id();
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
            let best = self.independent.pop_last()?;
            let hash = best.transaction.hash();

            // skip transactions that were marked as invalid
            if self.invalid.contains(hash) {
                debug!(
                    target: "txpool",
                    "[{:?}] skipping invalid transaction",
                    hash
                );
                continue
            }

            // Insert transactions that just got unlocked.
            if let Some(unlocked) = self.all.get(&best.unlocks()) {
                self.independent.insert(unlocked.clone());
            }

            if self.skip_blobs && best.transaction.transaction.is_eip4844() {
                // blobs should be skipped, marking the as invalid will ensure that no dependent
                // transactions are returned
                self.mark_invalid(&best.transaction)
            } else {
                return Some(best.transaction)
            }
        }
    }
}

/// A[`BestTransactions`](crate::traits::BestTransactions) implementation that filters the
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
    pub(crate) const fn new(best: I, predicate: P) -> Self {
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
}
