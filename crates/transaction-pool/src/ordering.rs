use crate::traits::PoolTransaction;
use std::fmt;
/// Transaction ordering trait to determine the order of transactions.
///
/// Decides how transactions should be ordered within the pool.
///
/// The returned priority must reflect natural `Ordering`
// TODO(mattsse) this should be extended so it provides a way to rank transaction in relation to
// each other.
pub trait TransactionOrdering: Send + Sync + 'static {
    /// Priority of a transaction.
    type Priority: Ord + Clone + Default + fmt::Debug + Send + Sync;

    /// The transaction type to score.
    type Transaction: PoolTransaction + Send + Sync + 'static;

    /// Returns the priority score for the given transaction.
    fn priority(&self, transaction: &Self::Transaction) -> Self::Priority;
}
