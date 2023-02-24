//! Error variants for the `eth_` namespace.

use crate::result::{internal_rpc_err, rpc_err};
use jsonrpsee::{core::Error as RpcError, types::error::INVALID_PARAMS_CODE};
use reth_primitives::U128;
use reth_rpc_types::BlockError;
use reth_transaction_pool::error::PoolError;
use revm::primitives::EVMError;

/// Result alias
pub(crate) type EthResult<T> = Result<T, EthApiError>;

/// List of JSON-RPC error codes
#[derive(Debug, Copy, PartialEq, Eq, Clone)]
pub(crate) enum EthRpcErrorCode {
    /// Failed to send transaction, See also <https://github.com/MetaMask/eth-rpc-errors/blob/main/src/error-constants.ts>
    TransactionRejected,
    /// Custom geth error code, <https://github.com/vapory-legacy/wiki/blob/master/JSON-RPC-Error-Codes-Improvement-Proposal.md>
    ExecutionError,
    /// <https://eips.ethereum.org/EIPS/eip-1898>
    InvalidInput,
}

impl EthRpcErrorCode {
    /// Returns the error code as `i32`
    pub(crate) const fn code(&self) -> i32 {
        match *self {
            EthRpcErrorCode::TransactionRejected => -32003,
            EthRpcErrorCode::ExecutionError => 3,
            EthRpcErrorCode::InvalidInput => -32000,
        }
    }
}

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
    /// An internal error where prevrandao is not set in the evm's environment
    #[error("Prevrandao not in th EVM's environment after merge")]
    PrevrandaoNotSet,
    #[error("Conflicting fee values in request. Both legacy gasPrice {gas_price} and maxFeePerGas {max_fee_per_gas} set")]
    ConflictingRequestGasPrice { gas_price: U128, max_fee_per_gas: U128 },
    #[error("Conflicting fee values in request. Both legacy gasPrice {gas_price} maxFeePerGas {max_fee_per_gas} and maxPriorityFeePerGas {max_priority_fee_per_gas} set")]
    ConflictingRequestGasPriceAndTipSet {
        gas_price: U128,
        max_fee_per_gas: U128,
        max_priority_fee_per_gas: U128,
    },
    #[error("Conflicting fee values in request. Legacy gasPrice {gas_price} and maxPriorityFeePerGas {max_priority_fee_per_gas} set")]
    RequestLegacyGasPriceAndTipSet { gas_price: U128, max_priority_fee_per_gas: U128 },
    #[error(transparent)]
    InvalidTransaction(#[from] InvalidTransactionError),
    /// Thrown when constructing an RPC block from a primitive block data failed.
    #[error(transparent)]
    InvalidBlockData(#[from] BlockError),
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
            EthApiError::InvalidTransaction(err) => err.into(),
            err => internal_rpc_err(err.to_string()),
        }
    }
}

impl<T> From<EVMError<T>> for EthApiError
where
    T: Into<EthApiError>,
{
    fn from(err: EVMError<T>) -> Self {
        match err {
            EVMError::Transaction(err) => InvalidTransactionError::from(err).into(),
            EVMError::PrevrandaoNotSet => EthApiError::PrevrandaoNotSet,
            EVMError::Database(err) => err.into(),
        }
    }
}

/// An error due to invalid transaction.
///
/// This adds compatibility with geth.
///
/// These error variants can be thrown when the transaction is checked prior to execution.
///
/// ## Nomenclature
///
/// This type is explicitly modeled after geth's error variants and uses
///   `fee cap` for `max_fee_per_gas`
///   `tip` for `max_priority_fee_per_gas`
#[derive(thiserror::Error, Debug)]
pub enum InvalidTransactionError {
    /// returned if the nonce of a transaction is lower than the one present in the local chain.
    #[error("nonce too low")]
    NonceTooLow,
    /// returned if the nonce of a transaction is higher than the next one expected based on the
    /// local chain.
    #[error("Nonce too high")]
    NonceTooHigh,
    /// Returned if the nonce of a transaction is too high
    /// Incrementing the nonce would lead to invalid state (overflow)
    #[error("nonce has max value")]
    NonceMaxValue,
    /// thrown if the transaction sender doesn't have enough funds for a transfer
    #[error("insufficient funds for transfer")]
    InsufficientFundsForTransfer,
    /// thrown if creation transaction provides the init code bigger than init code size limit.
    #[error("max initcode size exceeded")]
    MaxInitCodeSizeExceeded,
    /// Represents the inability to cover max cost + value (account balance too low).
    #[error("Insufficient funds for gas * price + value")]
    InsufficientFunds,
    /// Thrown when calculating gas usage
    #[error("gas uint64 overflow")]
    GasUintOverflow,
    /// returned if the transaction is specified to use less gas than required to start the
    /// invocation.
    #[error("intrinsic gas too low")]
    GasTooLow,
    /// returned if the transaction gas exceeds the limit
    #[error("intrinsic gas too high")]
    GasTooHigh,
    /// thrown if a transaction is not supported in the current network configuration.
    #[error("transaction type not supported")]
    TxTypeNotSupported,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    #[error("max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// A sanity error to avoid huge numbers specified in the tip field.
    #[error("max priority fee per gas higher than 2^256-1")]
    TipVeryHigh,
    /// A sanity error to avoid huge numbers specified in the fee cap field.
    #[error("max fee per gas higher than 2^256-1")]
    FeeCapVeryHigh,
    /// Thrown post London if the transaction's fee is less than the base fee of the block
    #[error("max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    #[error("sender not an eoa")]
    SenderNoEOA,
}

impl InvalidTransactionError {
    /// Returns the rpc error code for this error.
    fn error_code(&self) -> i32 {
        match self {
            InvalidTransactionError::GasTooLow | InvalidTransactionError::GasTooHigh => {
                EthRpcErrorCode::InvalidInput.code()
            }
            _ => EthRpcErrorCode::TransactionRejected.code(),
        }
    }
}

impl From<InvalidTransactionError> for RpcError {
    fn from(err: InvalidTransactionError) -> Self {
        rpc_err(err.error_code(), err.to_string(), None)
    }
}

impl From<revm::primitives::InvalidTransaction> for InvalidTransactionError {
    fn from(err: revm::primitives::InvalidTransaction) -> Self {
        use revm::primitives::InvalidTransaction;
        match err {
            InvalidTransaction::GasMaxFeeGreaterThanPriorityFee => {
                InvalidTransactionError::TipAboveFeeCap
            }
            InvalidTransaction::GasPriceLessThanBasefee => InvalidTransactionError::FeeCapTooLow,
            InvalidTransaction::CallerGasLimitMoreThanBlock => InvalidTransactionError::GasTooHigh,
            InvalidTransaction::CallGasCostMoreThanGasLimit => InvalidTransactionError::GasTooHigh,
            InvalidTransaction::RejectCallerWithCode => InvalidTransactionError::SenderNoEOA,
            InvalidTransaction::LackOfFundForGasLimit { .. } => {
                InvalidTransactionError::InsufficientFunds
            }
            InvalidTransaction::OverflowPaymentInTransaction => {
                InvalidTransactionError::GasUintOverflow
            }
            InvalidTransaction::NonceOverflowInTransaction => {
                InvalidTransactionError::NonceMaxValue
            }
            InvalidTransaction::CreateInitcodeSizeLimit => {
                InvalidTransactionError::MaxInitCodeSizeExceeded
            }
            InvalidTransaction::NonceTooHigh { .. } => InvalidTransactionError::NonceTooHigh,
            InvalidTransaction::NonceTooLow { .. } => InvalidTransactionError::NonceTooLow,
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
