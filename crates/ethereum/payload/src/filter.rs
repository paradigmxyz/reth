/// A filter that allows to check if a transaction satisfies a set of conditions
pub trait TransactionFilter {
    /// The type of the transaction to check.
    type Transaction;

    /// Returns true if the transaction satisfies the conditions.
    fn is_valid(&self, transaction: &Self::Transaction) -> bool;
}

/// A no-op implementation of [`TransactionFilter`] which
/// marks all transactions as valid.
#[derive(Debug)]
pub struct NoOpTransactionFilter<T>(std::marker::PhantomData<T>);

impl<T> NoOpTransactionFilter<T> {
    /// Creates a new [`NoOpTransactionFilter`].
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> TransactionFilter for NoOpTransactionFilter<T> {
    type Transaction = T;

    fn is_valid(&self, _transaction: &Self::Transaction) -> bool {
        true
    }
}
