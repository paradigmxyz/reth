//! Additional support for pooled transactions with [`TransactionInterop`]

/// Helper trait that allows attaching a [`TransactionInterop`].
pub trait MaybeInteropTransaction {
    /// Attach a [`TransactionInterop`].
    fn set_interop(&mut self, conditional: TransactionInterop);

    /// Get attached [`TransactionInterop`] if any.
    fn interop(&self) -> Option<&TransactionInterop>;

    /// Helper that sets the conditional and returns the instance again
    fn with_interop(mut self, interop: TransactionInterop) -> Self
    where
        Self: Sized,
    {
        self.set_interop(interop);
        self
    }
}

/// Helper to keep track of cross transaction interop validity
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TransactionInterop {
    /// Time until tx if validated successfully.
    /// If None - tx is not validated, tx should be automatically revalidated by interop tracker
    /// if timeout - current block timestamp < 1min
    // TODO: we should choose some concrete timing for revalidation, but 1min sound reasonable
    pub timeout: Option<u64>,
}

impl TransactionInterop {
    /// Checks if provided timestamp fits into tx validation window
    pub fn is_valid(&self, timestamp: u64) -> bool {
        if let Some(timeout) = self.timeout {
            if timestamp > timeout {
                return true;
            }
        }
        false
    }
}
