//! Additional support for estimating the data availability size of transactions.

/// Helper trait that allows attaching an estimated data availability size.
pub trait MaybeEstimatedDASize {
    /// Get the estimated data availability size of the transaction.
    fn estimated_da_size(&self) -> u64;
}
