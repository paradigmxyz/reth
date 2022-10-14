use crate::traits::PoolTransaction;
use std::fmt;

/// Transaction ordering trait to determine the order of transactions.
///
/// Decides how transactions should be ordered within the pool, depending on a `Priority` value.
///
/// The returned priority must reflect [total order](https://en.wikipedia.org/wiki/Total_order).
pub trait TransactionOrdering: Send + Sync + 'static {
    /// Priority of a transaction.
    ///
    /// Higher is better.
    type Priority: Ord + Clone + Default + fmt::Debug + Send + Sync;

    /// The transaction type to determine the priority of.
    type Transaction: PoolTransaction;

    /// Returns the priority score for the given transaction.
    fn priority(&self, transaction: &Self::Transaction) -> Self::Priority;
}
