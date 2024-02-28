use crate::{
    identifier::TransactionId, pool::pending::PendingTransaction, PoolTransaction,
    TransactionOrdering, ValidPoolTransaction,
};
use core::fmt;
use reth_primitives::B256 as TxHash;
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
            if best.transaction.max_fee_per_gas() < self.base_fee as u128 {
                // tx violates base fee, mark it as invalid and continue
                crate::traits::BestTransactions::mark_invalid(self, &best);
            } else {
                // tx is EIP4844 and violates blob fee, mark it as invalid and continue
                if best.transaction.max_fee_per_blob_gas().is_some_and(|max_fee_per_blob_gas| {
                    max_fee_per_blob_gas < self.base_fee_per_blob_gas as u128
                }) {
                    crate::traits::BestTransactions::mark_invalid(self, &best);
                    continue
                };
                return Some(best)
            }
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
    /// iterator was snapshotted
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

    /// Checks for new transactions that have come into the PendingPool after this iterator was
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
        BestTransactions::mark_invalid(self, tx)
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
/// [BestTransactions::mark_invalid](crate::traits::BestTransactions::mark_invalid).
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
            } else {
                self.best.mark_invalid(&best);
            }
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
    };

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
}
