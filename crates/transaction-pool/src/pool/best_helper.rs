#![allow(dead_code)]
use std::sync::Arc;

use crate::{pool::best::BestTransactions, TransactionOrdering, ValidPoolTransaction};

pub(crate) enum InvalidPatterns {
    // prevent state change leakage by private transactions
    PrivateTransaction,
}

pub(crate) struct BestTransactionsHelper<T: TransactionOrdering> {
    pub(crate) best: BestTransactions<T>,
    pub(crate) invalid_patterns: Vec<InvalidPatterns>,
}

impl<T: TransactionOrdering> Iterator for BestTransactionsHelper<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let best = self.best.next()?;
            if self.check_if_tx_invalid(best.clone()) {
                self.best.mark_invalid(&best);
            } else {
                return Some(best);
            }
        }
    }
}

impl<T: TransactionOrdering> BestTransactionsHelper<T> {
    pub(crate) fn check_if_tx_invalid(
        &self,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
    ) -> bool {
        self.invalid_patterns.iter().any(|pattern| match pattern {
            InvalidPatterns::PrivateTransaction => tx.origin.is_private(),
        })
    }

    pub(crate) fn add_invalid_pattern(&mut self, pattern: InvalidPatterns) {
        self.invalid_patterns.push(pattern);
    }
}
impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactionsHelper<T> {
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
