use derive_more::Display;
use reth_optimism_primitives::supervisor::{
    InteropTxValidatorError, InvalidInboxEntry, SafetyLevel,
};
use reth_transaction_pool::error::PoolTransactionError;
use std::any::Any;

/// Wrapper for [`InteropTxValidatorError`] to implement [`PoolTransactionError`] for it.
#[derive(thiserror::Error, Debug, Display)]
pub struct InvalidCrossTx(InteropTxValidatorError);

impl InvalidCrossTx {
    /// Creates instance of [`InvalidCrossTx`] from [`InteropTxValidatorError`]
    pub const fn new(error: InteropTxValidatorError) -> Self {
        Self(error)
    }
}
impl PoolTransactionError for InvalidCrossTx {
    fn is_bad_transaction(&self) -> bool {
        match &self.0 {
            // TODO: potentially tx could become valid after a while, then
            InteropTxValidatorError::InvalidInboxEntry(err) => match err {
                // This transaction could become valid after a while
                InvalidInboxEntry::MinimumSafety { got, .. } => match got {
                    // This transaction will never become valid
                    SafetyLevel::Invalid => true,
                    // This transaction will become valid when origin chain progress
                    // TODO: it's not completely the case for Unsafe level, bogus transaction could
                    // pass unsafe check but will not become valid. And valid
                    // transaction could start with unsafe and progress as
                    // origin chain progresses. NOTE: this was behavior for
                    // previous
                    _ => false,
                },
                // This tx will not become valid unless supervisor is reconfigured
                InvalidInboxEntry::UnknownChain(_) => true,
            },
            // Rpc error or supervisor haven't responded in time
            InteropTxValidatorError::RpcClientError(_) |
            InteropTxValidatorError::ValidationTimeout(_) => false,
            // Transaction caused unknown (for parsing) error in supervisor
            InteropTxValidatorError::SupervisorServerError(_) => true,
        }
    }

    fn as_any(&self) -> &dyn Any {
        &self.0
    }
}
