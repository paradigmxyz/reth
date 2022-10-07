use crate::traits::PoolTransaction;
use std::fmt;

/// Transaction ordering.
///
/// Decides how transactions should be ordered within the pool.
///
/// The returned priority must reflect natural `Ordering`.
// TODO: for custom, more advanced scoring it would be ideal to determine the priority in the
// context of the entire pool instead of standalone by alone looking at a single transaction
pub trait TransactionOrdering: Send + Sync + 'static {
    /// Priority of a transaction.
    type Priority: Ord + Clone + Default + fmt::Debug + Send + Sync;

    /// The transaction type to score.
    type Transaction: PoolTransaction + Send + Sync + 'static;

    /// Returns the priority score for the given transaction.
    fn priority(&self, transaction: &Self::Transaction) -> Self::Priority;
}
