use std::fmt;

/// Transaction ordering.
///
/// Decides how transactions should be ordered within the pool.
///
/// The returned priority must reflect natural `Ordering`.
pub trait TransactionOrdering: Send + Sync {
    /// Priority of a transaction.
    type Priority: Ord + Clone + Default + fmt::Debug + fmt::LowerHex + Send + Sync;

    /// Returns the priority score for the given transaction.
    fn priority(&self, transaction: ()) -> Self::Priority;
}
