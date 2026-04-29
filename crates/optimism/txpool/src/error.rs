use crate::supervisor::InteropTxValidatorError;
use reth_transaction_pool::error::PoolTransactionError;
use std::any::Any;

/// Wrapper for [`InteropTxValidatorError`] to implement [`PoolTransactionError`] for it.
#[derive(thiserror::Error, Debug)]
pub enum InvalidCrossTx {
    /// Errors produced by supervisor validation
    #[error(transparent)]
    ValidationError(#[from] InteropTxValidatorError),
    /// Error cause by cross chain tx during not active interop hardfork
    #[error("cross chain tx is invalid before interop")]
    CrossChainTxPreInterop,
}

impl PoolTransactionError for InvalidCrossTx {
    fn is_bad_transaction(&self) -> bool {
        match self {
            Self::ValidationError(_) => false,
            Self::CrossChainTxPreInterop => true,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Mantle `MetaTx` has been permanently disabled since the `MantleEverest` hardfork.
/// Mirrors op-geth: `ErrMetaTxDisabled` in `core/types/meta_transaction.go`.
#[derive(thiserror::Error, Debug)]
#[error("meta tx is disabled")]
pub struct MetaTxDisabled;

impl PoolTransactionError for MetaTxDisabled {
    fn is_bad_transaction(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
