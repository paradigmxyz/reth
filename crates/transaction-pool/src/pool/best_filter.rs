#![allow(dead_code)]
use std::sync::Arc;

use crate::{pool::best::BestTransactions, TransactionOrdering, ValidPoolTransaction};

pub(crate) struct BestTransactionFilter<T: TransactionOrdering, P> {
    pub(crate) best: BestTransactions<T>,
    pub(crate) predicate: P,
}

impl<T, P> BestTransactionFilter<T, P>
where
    T: TransactionOrdering,
    P: FnMut(&Arc<ValidPoolTransaction<T::Transaction>>) -> bool,
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
