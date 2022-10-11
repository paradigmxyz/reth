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

/// An iterator that returns transactions that can be executed on the current state.
pub struct BestTransactions<T: TransactionOrdering> {
    pub(crate) all: BTreeMap<TransactionId, Arc<PendingTransaction<T>>>,
    pub(crate) independent: BTreeSet<PendingTransactionRef<T>>,
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
