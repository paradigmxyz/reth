//! Error variants for the `eth_` namespace.

use jsonrpsee::{core::Error as RpcError, types::error::INVALID_PARAMS_CODE};
use reth_transaction_pool::error::PoolError;

use crate::result::{internal_rpc_err, rpc_err};

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
    PoolError(GethTxPoolError),
    #[error("Unknown block number")]
    UnknownBlockNumber,
    #[error("Invalid block range")]
    InvalidBlockRange,
    /// Other internal error
    #[error(transparent)]
    Internal(#[from] reth_interfaces::Error),
}

impl From<EthApiError> for RpcError {
    fn from(value: EthApiError) -> Self {
        match value {
            EthApiError::FailedToDecodeSignedTransaction |
            EthApiError::InvalidTransactionSignature |
            EthApiError::EmptyRawTransactionData |
            EthApiError::UnknownBlockNumber |
            EthApiError::InvalidBlockRange => rpc_err(INVALID_PARAMS_CODE, value.to_string(), None),
            err => internal_rpc_err(err.to_string()),
        }
    }
}

/// A helper error type that mirrors `geth` Txpool's error messages
#[derive(Debug, thiserror::Error)]
pub(crate) enum GethTxPoolError {
    #[error("already known")]
    AlreadyKnown,
    #[error("invalid sender")]
    InvalidSender,
    #[error("transaction underpriced")]
    Underpriced,
    #[error("txpool is full")]
    TxPoolOverflow,
    #[error("replacement transaction underpriced")]
    ReplaceUnderpriced,
    #[error("exceeds block gas limit")]
    GasLimit,
    #[error("negative value")]
    NegativeValue,
    #[error("oversized data")]
    OversizedData,
}

impl From<PoolError> for GethTxPoolError {
    fn from(err: PoolError) -> GethTxPoolError {
        match err {
            PoolError::ReplacementUnderpriced(_) => GethTxPoolError::ReplaceUnderpriced,
            PoolError::ProtocolFeeCapTooLow(_, _) => GethTxPoolError::Underpriced,
            PoolError::SpammerExceededCapacity(_, _) => GethTxPoolError::TxPoolOverflow,
            PoolError::DiscardedOnInsert(_) => GethTxPoolError::TxPoolOverflow,
            PoolError::TxExceedsGasLimit(_, _, _) => GethTxPoolError::GasLimit,
            PoolError::TxExceedsMaxInitCodeSize(_, _, _) => GethTxPoolError::OversizedData,
        }
    }
}

impl From<PoolError> for EthApiError {
    fn from(err: PoolError) -> Self {
        EthApiError::PoolError(GethTxPoolError::from(err))
    }
}
