use crate::{
    identifier::TransactionId,
    pool::pending::{PendingTransaction, PendingTransactionRef},
    TransactionOrdering, ValidPoolTransaction,
};
use reth_primitives::H256 as TxHash;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::Arc,
};
use tracing::debug;

/// An iterator that returns transactions that can be executed on the current state (*best*
/// transactions).
///
/// The [`PendingPool`] contains transactions that *could* all be executed on the current state, but
/// only yields transactions that are ready to be executed now.
/// While it contains all gapless transactions of a sender, it _always_ only returns the transaction
/// with the current on chain nonce.
pub struct BestTransactions<T: TransactionOrdering> {
    /// Contains a copy of _all_ transactions of the pending pool at the point in time this
    /// iterator was created.
    pub(crate) all: BTreeMap<TransactionId, Arc<PendingTransaction<T>>>,
    /// Transactions that can be executed right away: these have the expected nonce.
    ///
    /// Once an `independent` transaction with the nonce `N` is returned, it unlocks `N+1`, which
    /// then can be moved from the `all` set to the `independent` set.
    pub(crate) independent: BTreeSet<PendingTransactionRef<T>>,
    /// There might be the case where a yielded transactions is invalid, this will track it.
    pub(crate) invalid: HashSet<TxHash>,
}

impl<T: TransactionOrdering> BestTransactions<T> {
    /// Mark the transaction and it's descendants as invalid.
    pub(crate) fn mark_invalid(&mut self, tx: &Arc<ValidPoolTransaction<T::Transaction>>) {
        self.invalid.insert(*tx.hash());
    }
}

impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item) {
        BestTransactions::mark_invalid(self, tx)
    }
}

impl<T: TransactionOrdering> Iterator for BestTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Remove the next independent tx with the highest priority
            let best = self.independent.iter().next_back()?.clone();
            let best = self.independent.take(&best)?;
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
                self.independent.insert(unlocked.transaction.clone());
            }

            return Some(best.transaction)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pool::pending::PendingPool,
        test_util::{MockOrdering, MockTransaction, MockTransactionFactory},
    };

    #[test]
    fn test_best_iter() {
        let mut pool = PendingPool::new(Arc::new(MockOrdering::default()));
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx));
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
        let mut pool = PendingPool::new(Arc::new(MockOrdering::default()));
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx));
        }

        let mut best = pool.best();

        // mark the first tx as invalid
        let invalid = best.independent.iter().next().unwrap();
        best.mark_invalid(&invalid.transaction.clone());

        // iterator is empty
        assert!(best.next().is_none());
    }
}
