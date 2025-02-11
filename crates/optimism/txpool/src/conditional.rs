//! Additional support for pooled transactions with [`TransactionConditional`]

use alloy_rpc_types_eth::erc4337::TransactionConditional;

/// Helper trait that allows attaching a [`TransactionConditional`].
pub trait MaybeConditionalTransaction {
    /// Attach a [`TransactionConditional`].
    fn set_conditional(&mut self, conditional: TransactionConditional);

    /// Helper that sets the conditional and returns the instance again
    fn with_conditional(mut self, conditional: TransactionConditional) -> Self
    where
        Self: Sized,
    {
        self.set_conditional(conditional);
        self
    }
}
