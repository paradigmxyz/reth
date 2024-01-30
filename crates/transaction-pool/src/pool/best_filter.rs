#![allow(dead_code)]
use std::sync::Arc;

use crate::{pool::best::BestTransactions, TransactionOrdering, ValidPoolTransaction};

/// A wrapper around [`BestTransactions`] that also filters transactions based on a predicate.
pub(crate) struct BestTransactionFilter<T: TransactionOrdering, P> {
    pub(crate) best: BestTransactions<T>,
    pub(crate) predicate: P,
}

impl<T, P> BestTransactionFilter<T, P>
where
    T: TransactionOrdering,
{
    pub(crate) fn new(best: BestTransactions<T>, predicate: P) -> Self {
        Self { best, predicate }
    }
}

impl<T: TransactionOrdering, P> Iterator for BestTransactionFilter<T, P>
where
    P: FnMut(&Arc<ValidPoolTransaction<T::Transaction>>) -> bool,
{
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let best = self.best.next()?;
            if (self.predicate)(&best) {
                return Some(best);
            } else {
                self.best.mark_invalid(&best);
            }
        }
    }
}

impl<T: TransactionOrdering, P> crate::traits::BestTransactions for BestTransactionFilter<T, P>
where
    P: FnMut(&Arc<ValidPoolTransaction<T::Transaction>>) -> bool + std::marker::Send,
{
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
