//! Error variants for the `eth_` namespace.

use crate::{impl_to_rpc_result, result::ToRpcResult};
use reth_transaction_pool::error::PoolError;

/// Result alias
pub(crate) type EthResult<T> = Result<T, EthApiError>;

/// Errors that can occur when interacting with the `eth_` namespace
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub(crate) enum EthApiError {
    /// When a raw transaction is empty
    #[error("Empty transaction data")]
    EmptyRawTransactionData,
    #[error("Failed to decode signed transaction")]
    FailedToDecodeSignedTransaction,
    #[error("Invalid transaction signature")]
    InvalidTransactionSignature,
    #[error(transparent)]
    PoolError(GethCompatPoolError),
}

impl_to_rpc_result!(EthApiError);

/// A helper error type that ensures error messages are compatible with `geth`
// TODO: replace thiserror with custom convert
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct GethCompatPoolError(PoolError);

impl From<PoolError> for EthApiError {
    fn from(err: PoolError) -> Self {
        EthApiError::PoolError(GethCompatPoolError(err))
    }
}
