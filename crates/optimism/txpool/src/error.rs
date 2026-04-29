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

/// Unprotected (non-EIP-155) legacy transactions are rejected by default.
///
/// Legacy transactions signed without replay protection (v=27 or v=28) have no chain ID
/// in the signature and can be replayed on any chain. op-geth rejects these by default
/// at the RPC layer (`AllowUnprotectedTxs=false`). This error provides the equivalent
/// enforcement in reth's transaction pool validator.
///
/// Mirrors op-geth: `internal/ethapi/api.go` — `SubmitTransaction()` guard.
#[derive(thiserror::Error, Debug)]
#[error("only replay-protected (EIP-155) transactions allowed")]
pub struct UnprotectedTxDisabled;

impl PoolTransactionError for UnprotectedTxDisabled {
    fn is_bad_transaction(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
