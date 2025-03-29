//! Additional support for pooled transactions with [`TransactionInterop`]

/// Helper trait that allows attaching a [`TransactionInterop`].
pub trait MaybeInteropTransaction {
    /// Attach a [`TransactionInterop`].
    fn set_interop(&self, deadline: TransactionInterop);

    /// Get attached [`TransactionInterop`] if any.
    fn interop(&self) -> Option<TransactionInterop>;

    /// Helper that sets the interop and returns the instance again
    fn with_interop(self, interop: TransactionInterop) -> Self
    where
        Self: Sized,
    {
        self.set_interop(interop);
        self
    }
}

/// Helper to keep track of cross transaction interop validity
pub type TransactionInterop = u64;
/// Checks if provided timestamp fits into tx validation window
#[inline]
pub fn is_valid(timeout: TransactionInterop, timestamp: u64) -> bool {
    timestamp < timeout
}

/// Checks if transaction needs revalidation based on offset
#[inline]
pub fn is_stale(timeout: TransactionInterop, timestamp: u64, offset: u64) -> bool {
    timestamp + offset > timeout
}
