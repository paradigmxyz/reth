//! Implementation specific Errors for the `eth_` namespace.

use crate::result::{internal_rpc_err, invalid_params_rpc_err, rpc_err, rpc_error_with_code};
use jsonrpsee::{
    core::Error as RpcError,
    types::{error::CALL_EXECUTION_FAILED_CODE, ErrorObject},
};
use reth_primitives::{abi::decode_revert_reason, Address, Bytes, U256};
use reth_revm::tracing::js::JsInspectorError;
use reth_rpc_types::{error::EthRpcErrorCode, BlockError, CallInputError};
use reth_transaction_pool::error::{
    Eip4844PoolTransactionError, InvalidPoolTransactionError, PoolError, PoolTransactionError,
};
use revm::primitives::{EVMError, ExecutionResult, Halt, OutOfGasError};
use std::time::Duration;

/// Result alias
pub type EthResult<T> = Result<T, EthApiError>;

/// Errors that can occur when interacting with the `eth_` namespace
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum EthApiError {
    /// When a raw transaction is empty
    #[error("Empty transaction data")]
    EmptyRawTransactionData,
    #[error("Failed to decode signed transaction")]
    FailedToDecodeSignedTransaction,
    #[error("Invalid transaction signature")]
    InvalidTransactionSignature,
    #[error(transparent)]
    PoolError(RpcPoolError),
    #[error("Unknown block number")]
    UnknownBlockNumber,
    /// Thrown when querying for `finalized` or `safe` block before the merge transition is
    /// finalized, <https://github.com/ethereum/execution-apis/blob/6d17705a875e52c26826124c2a8a15ed542aeca2/src/schemas/block.yaml#L109>
    #[error("Unknown block")]
    UnknownSafeOrFinalizedBlock,
    #[error("Unknown block or tx index")]
    UnknownBlockOrTxIndex,
    #[error("Invalid block range")]
    InvalidBlockRange,
    /// An internal error where prevrandao is not set in the evm's environment
    #[error("Prevrandao not in th EVM's environment after merge")]
    PrevrandaoNotSet,
    /// Thrown when a call or transaction request (`eth_call`, `eth_estimateGas`,
    /// `eth_sendTransaction`) contains conflicting fields (legacy, EIP-1559)
    #[error("both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified")]
    ConflictingFeeFieldsInRequest,
    #[error(transparent)]
    InvalidTransaction(#[from] RpcInvalidTransactionError),
    /// Thrown when constructing an RPC block from a primitive block data failed.
    #[error(transparent)]
    InvalidBlockData(#[from] BlockError),
    /// Thrown when a [AccountOverride](reth_rpc_types::state::AccountOverride) contains
    /// conflicting `state` and `stateDiff` fields
    #[error("account {0:?} has both 'state' and 'stateDiff'")]
    BothStateAndStateDiffInOverride(Address),
    /// Other internal error
    #[error(transparent)]
    Internal(reth_interfaces::Error),
    /// Error related to signing
    #[error(transparent)]
    Signing(#[from] SignError),
    /// Thrown when a transaction was requested but not matching transaction exists
    #[error("transaction not found")]
    TransactionNotFound,
    /// Some feature is unsupported
    #[error("unsupported")]
    Unsupported(&'static str),
    /// General purpose error for invalid params
    #[error("{0}")]
    InvalidParams(String),
    /// When tracer config does not match the tracer
    #[error("invalid tracer config")]
    InvalidTracerConfig,
    /// Percentile array is invalid
    #[error("invalid reward percentiles")]
    InvalidRewardPercentiles,
    /// Error thrown when a spawned tracing task failed to deliver an anticipated response.
    ///
    /// This only happens if the tracing task panics and is aborted before it can return a response
    /// back to the request handler.
    #[error("internal error while tracing")]
    InternalTracingError,
    /// Error thrown when a spawned blocking task failed to deliver an anticipated response.
    #[error("internal eth error")]
    InternalEthError,
    /// Error thrown when a (tracing) call exceeded the configured timeout.
    #[error("execution aborted (timeout = {0:?})")]
    ExecutionTimedOut(Duration),
    /// Internal Error thrown by the javascript tracer
    #[error("{0}")]
    InternalJsTracerError(String),
    #[error(transparent)]
    CallInputError(#[from] CallInputError),
}

impl From<EthApiError> for ErrorObject<'static> {
    fn from(error: EthApiError) -> Self {
        match error {
            EthApiError::FailedToDecodeSignedTransaction |
            EthApiError::InvalidTransactionSignature |
            EthApiError::EmptyRawTransactionData |
            EthApiError::InvalidBlockRange |
            EthApiError::ConflictingFeeFieldsInRequest |
            EthApiError::Signing(_) |
            EthApiError::BothStateAndStateDiffInOverride(_) |
            EthApiError::InvalidTracerConfig => invalid_params_rpc_err(error.to_string()),
            EthApiError::InvalidTransaction(err) => err.into(),
            EthApiError::PoolError(err) => err.into(),
            EthApiError::PrevrandaoNotSet |
            EthApiError::InvalidBlockData(_) |
            EthApiError::Internal(_) |
            EthApiError::TransactionNotFound => internal_rpc_err(error.to_string()),
            EthApiError::UnknownBlockNumber | EthApiError::UnknownBlockOrTxIndex => {
                rpc_error_with_code(EthRpcErrorCode::ResourceNotFound.code(), error.to_string())
            }
            EthApiError::UnknownSafeOrFinalizedBlock => {
                rpc_error_with_code(EthRpcErrorCode::UnknownBlock.code(), error.to_string())
            }
            EthApiError::Unsupported(msg) => internal_rpc_err(msg),
            EthApiError::InternalJsTracerError(msg) => internal_rpc_err(msg),
            EthApiError::InvalidParams(msg) => invalid_params_rpc_err(msg),
            EthApiError::InvalidRewardPercentiles => internal_rpc_err(error.to_string()),
            err @ EthApiError::ExecutionTimedOut(_) => {
                rpc_error_with_code(CALL_EXECUTION_FAILED_CODE, err.to_string())
            }
            err @ EthApiError::InternalTracingError => internal_rpc_err(err.to_string()),
            err @ EthApiError::InternalEthError => internal_rpc_err(err.to_string()),
            err @ EthApiError::CallInputError(_) => invalid_params_rpc_err(err.to_string()),
        }
    }
}

impl From<EthApiError> for RpcError {
    fn from(error: EthApiError) -> Self {
        RpcError::Call(error.into())
    }
}
impl From<JsInspectorError> for EthApiError {
    fn from(error: JsInspectorError) -> Self {
        match error {
            err @ JsInspectorError::JsError(_) => {
                EthApiError::InternalJsTracerError(err.to_string())
            }
            err => EthApiError::InvalidParams(err.to_string()),
        }
    }
}

impl From<reth_interfaces::Error> for EthApiError {
    fn from(error: reth_interfaces::Error) -> Self {
        match error {
            reth_interfaces::Error::Provider(err) => err.into(),
            err => EthApiError::Internal(err),
        }
    }
}

impl From<reth_interfaces::provider::ProviderError> for EthApiError {
    fn from(error: reth_interfaces::provider::ProviderError) -> Self {
        use reth_interfaces::provider::ProviderError;
        match error {
            ProviderError::HeaderNotFound(_) |
            ProviderError::BlockHashNotFound(_) |
            ProviderError::BestBlockNotFound |
            ProviderError::BlockNumberForTransactionIndexNotFound |
            ProviderError::TotalDifficultyNotFound { .. } |
            ProviderError::UnknownBlockHash(_) => EthApiError::UnknownBlockNumber,
            ProviderError::FinalizedBlockNotFound | ProviderError::SafeBlockNotFound => {
                EthApiError::UnknownSafeOrFinalizedBlock
            }
            err => EthApiError::Internal(err.into()),
        }
    }
}

impl<T> From<EVMError<T>> for EthApiError
where
    T: Into<EthApiError>,
{
    fn from(err: EVMError<T>) -> Self {
        match err {
            EVMError::Transaction(err) => RpcInvalidTransactionError::from(err).into(),
            EVMError::PrevrandaoNotSet => EthApiError::PrevrandaoNotSet,
            EVMError::Database(err) => err.into(),
        }
    }
}

/// An error due to invalid transaction.
///
/// The only reason this exists is to maintain compatibility with other clients de-facto standard
/// error messages.
///
/// These error variants can be thrown when the transaction is checked prior to execution.
///
/// These variants also cover all errors that can be thrown by revm.
///
/// ## Nomenclature
///
/// This type is explicitly modeled after geth's error variants and uses
///   `fee cap` for `max_fee_per_gas`
///   `tip` for `max_priority_fee_per_gas`
#[derive(thiserror::Error, Debug)]
pub enum RpcInvalidTransactionError {
    /// returned if the nonce of a transaction is lower than the one present in the local chain.
    #[error("nonce too low")]
    NonceTooLow,
    /// returned if the nonce of a transaction is higher than the next one expected based on the
    /// local chain.
    #[error("nonce too high")]
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
    #[error("insufficient funds for gas * price + value")]
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
    /// Thrown during estimate if caller has insufficient funds to cover the tx.
    #[error("Out of gas: gas required exceeds allowance: {0:?}")]
    BasicOutOfGas(U256),
    /// As BasicOutOfGas but thrown when gas exhausts during memory expansion.
    #[error("Out of gas: gas exhausts during memory expansion: {0:?}")]
    MemoryOutOfGas(U256),
    /// As BasicOutOfGas but thrown when gas exhausts during precompiled contract execution.
    #[error("Out of gas: gas exhausts during precompiled contract execution: {0:?}")]
    PrecompileOutOfGas(U256),
    /// revm's Type cast error, U256 casts down to a u64 with overflow
    #[error("Out of gas: revm's Type cast error, U256 casts down to a u64 with overflow {0:?}")]
    InvalidOperandOutOfGas(U256),
    /// Thrown if executing a transaction failed during estimate/call
    #[error("{0}")]
    Revert(RevertError),
    /// Unspecific evm halt error
    #[error("EVM error {0:?}")]
    EvmHalt(Halt),
    /// Invalid chain id set for the transaction.
    #[error("Invalid chain id")]
    InvalidChainId,
    /// The transaction is before Spurious Dragon and has a chain ID
    #[error("Transactions before Spurious Dragon should not have a chain ID.")]
    OldLegacyChainId,
}

impl RpcInvalidTransactionError {
    /// Returns the rpc error code for this error.
    fn error_code(&self) -> i32 {
        match self {
            RpcInvalidTransactionError::InvalidChainId |
            RpcInvalidTransactionError::GasTooLow |
            RpcInvalidTransactionError::GasTooHigh => EthRpcErrorCode::InvalidInput.code(),
            RpcInvalidTransactionError::Revert(_) => EthRpcErrorCode::ExecutionError.code(),
            _ => EthRpcErrorCode::TransactionRejected.code(),
        }
    }

    /// Converts the halt error
    ///
    /// Takes the configured gas limit of the transaction which is attached to the error
    pub(crate) fn halt(reason: Halt, gas_limit: u64) -> Self {
        match reason {
            Halt::OutOfGas(err) => RpcInvalidTransactionError::out_of_gas(err, gas_limit),
            Halt::NonceOverflow => RpcInvalidTransactionError::NonceMaxValue,
            err => RpcInvalidTransactionError::EvmHalt(err),
        }
    }

    /// Converts the out of gas error
    pub(crate) fn out_of_gas(reason: OutOfGasError, gas_limit: u64) -> Self {
        let gas_limit = U256::from(gas_limit);
        match reason {
            OutOfGasError::BasicOutOfGas => RpcInvalidTransactionError::BasicOutOfGas(gas_limit),
            OutOfGasError::Memory => RpcInvalidTransactionError::MemoryOutOfGas(gas_limit),
            OutOfGasError::Precompile => RpcInvalidTransactionError::PrecompileOutOfGas(gas_limit),
            OutOfGasError::InvalidOperand => {
                RpcInvalidTransactionError::InvalidOperandOutOfGas(gas_limit)
            }
            OutOfGasError::MemoryLimit => RpcInvalidTransactionError::MemoryOutOfGas(gas_limit),
        }
    }
}

impl From<RpcInvalidTransactionError> for ErrorObject<'static> {
    fn from(err: RpcInvalidTransactionError) -> Self {
        match err {
            RpcInvalidTransactionError::Revert(revert) => {
                // include out data if some
                rpc_err(
                    revert.error_code(),
                    revert.to_string(),
                    revert.output.as_ref().map(|out| out.as_ref()),
                )
            }
            err => rpc_err(err.error_code(), err.to_string(), None),
        }
    }
}

impl From<revm::primitives::InvalidTransaction> for RpcInvalidTransactionError {
    fn from(err: revm::primitives::InvalidTransaction) -> Self {
        use revm::primitives::InvalidTransaction;
        match err {
            InvalidTransaction::InvalidChainId => RpcInvalidTransactionError::InvalidChainId,
            InvalidTransaction::GasMaxFeeGreaterThanPriorityFee => {
                RpcInvalidTransactionError::TipAboveFeeCap
            }
            InvalidTransaction::GasPriceLessThanBasefee => RpcInvalidTransactionError::FeeCapTooLow,
            InvalidTransaction::CallerGasLimitMoreThanBlock => {
                RpcInvalidTransactionError::GasTooHigh
            }
            InvalidTransaction::CallGasCostMoreThanGasLimit => {
                RpcInvalidTransactionError::GasTooHigh
            }
            InvalidTransaction::RejectCallerWithCode => RpcInvalidTransactionError::SenderNoEOA,
            InvalidTransaction::LackOfFundForGasLimit { .. } => {
                RpcInvalidTransactionError::InsufficientFunds
            }
            InvalidTransaction::OverflowPaymentInTransaction => {
                RpcInvalidTransactionError::GasUintOverflow
            }
            InvalidTransaction::NonceOverflowInTransaction => {
                RpcInvalidTransactionError::NonceMaxValue
            }
            InvalidTransaction::CreateInitcodeSizeLimit => {
                RpcInvalidTransactionError::MaxInitCodeSizeExceeded
            }
            InvalidTransaction::NonceTooHigh { .. } => RpcInvalidTransactionError::NonceTooHigh,
            InvalidTransaction::NonceTooLow { .. } => RpcInvalidTransactionError::NonceTooLow,
        }
    }
}

impl From<reth_primitives::InvalidTransactionError> for RpcInvalidTransactionError {
    fn from(err: reth_primitives::InvalidTransactionError) -> Self {
        use reth_primitives::InvalidTransactionError;
        // This conversion is used to convert any transaction errors that could occur inside the
        // txpool (e.g. `eth_sendRawTransaction`) to their corresponding RPC
        match err {
            InvalidTransactionError::InsufficientFunds { .. } => {
                RpcInvalidTransactionError::InsufficientFunds
            }
            InvalidTransactionError::NonceNotConsistent => RpcInvalidTransactionError::NonceTooLow,
            InvalidTransactionError::OldLegacyChainId => {
                // Note: this should be unreachable since Spurious Dragon now enabled
                RpcInvalidTransactionError::OldLegacyChainId
            }
            InvalidTransactionError::ChainIdMismatch => RpcInvalidTransactionError::InvalidChainId,
            InvalidTransactionError::Eip2930Disabled |
            InvalidTransactionError::Eip1559Disabled |
            InvalidTransactionError::Eip4844Disabled => {
                RpcInvalidTransactionError::TxTypeNotSupported
            }
            InvalidTransactionError::TxTypeNotSupported => {
                RpcInvalidTransactionError::TxTypeNotSupported
            }
            InvalidTransactionError::GasUintOverflow => RpcInvalidTransactionError::GasUintOverflow,
            InvalidTransactionError::GasTooLow => RpcInvalidTransactionError::GasTooLow,
            InvalidTransactionError::GasTooHigh => RpcInvalidTransactionError::GasTooHigh,
            InvalidTransactionError::TipAboveFeeCap => RpcInvalidTransactionError::TipAboveFeeCap,
            InvalidTransactionError::FeeCapTooLow => RpcInvalidTransactionError::FeeCapTooLow,
            InvalidTransactionError::SignerAccountHasBytecode => {
                RpcInvalidTransactionError::SenderNoEOA
            }
        }
    }
}

/// Represents a reverted transaction and its output data.
///
/// Displays "execution reverted(: reason)?" if the reason is a string.
#[derive(Debug, Clone)]
pub struct RevertError {
    /// The transaction output data
    ///
    /// Note: this is `None` if output was empty
    output: Option<bytes::Bytes>,
}

// === impl RevertError ==

impl RevertError {
    /// Wraps the output bytes
    ///
    /// Note: this is intended to wrap an revm output
    pub fn new(output: bytes::Bytes) -> Self {
        if output.is_empty() {
            Self { output: None }
        } else {
            Self { output: Some(output) }
        }
    }

    fn error_code(&self) -> i32 {
        EthRpcErrorCode::ExecutionError.code()
    }
}

impl std::fmt::Display for RevertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("execution reverted")?;
        if let Some(reason) = self.output.as_ref().and_then(decode_revert_reason) {
            write!(f, ": {reason}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RevertError {}

/// A helper error type that's mainly used to mirror `geth` Txpool's error messages
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum RpcPoolError {
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
    ExceedsGasLimit,
    #[error("negative value")]
    NegativeValue,
    #[error("oversized data")]
    OversizedData,
    #[error("max initcode size exceeded")]
    ExceedsMaxInitCodeSize,
    #[error(transparent)]
    Invalid(#[from] RpcInvalidTransactionError),
    /// Custom pool error
    #[error("{0:?}")]
    PoolTransactionError(Box<dyn PoolTransactionError>),
    /// Eip-4844 related error
    #[error(transparent)]
    Eip4844(#[from] Eip4844PoolTransactionError),
    /// Thrown if a conflicting transaction type is already in the pool
    ///
    /// In other words, thrown if a transaction with the same sender that violates the exclusivity
    /// constraint (blob vs normal tx)
    #[error("address already reserved")]
    AddressAlreadyReserved,
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl From<RpcPoolError> for ErrorObject<'static> {
    fn from(error: RpcPoolError) -> Self {
        match error {
            RpcPoolError::Invalid(err) => err.into(),
            error => internal_rpc_err(error.to_string()),
        }
    }
}

impl From<PoolError> for RpcPoolError {
    fn from(err: PoolError) -> RpcPoolError {
        match err {
            PoolError::ReplacementUnderpriced(_) => RpcPoolError::ReplaceUnderpriced,
            PoolError::FeeCapBelowMinimumProtocolFeeCap(_, _) => RpcPoolError::Underpriced,
            PoolError::SpammerExceededCapacity(_, _) => RpcPoolError::TxPoolOverflow,
            PoolError::DiscardedOnInsert(_) => RpcPoolError::TxPoolOverflow,
            PoolError::InvalidTransaction(_, err) => err.into(),
            PoolError::Other(_, err) => RpcPoolError::Other(err),
            PoolError::AlreadyImported(_) => RpcPoolError::AlreadyKnown,
            PoolError::ExistingConflictingTransactionType(_, _, _) => {
                RpcPoolError::AddressAlreadyReserved
            }
        }
    }
}

impl From<InvalidPoolTransactionError> for RpcPoolError {
    fn from(err: InvalidPoolTransactionError) -> RpcPoolError {
        match err {
            InvalidPoolTransactionError::Consensus(err) => RpcPoolError::Invalid(err.into()),
            InvalidPoolTransactionError::ExceedsGasLimit(_, _) => RpcPoolError::ExceedsGasLimit,
            InvalidPoolTransactionError::ExceedsMaxInitCodeSize(_, _) => {
                RpcPoolError::ExceedsMaxInitCodeSize
            }
            InvalidPoolTransactionError::OversizedData(_, _) => RpcPoolError::OversizedData,
            InvalidPoolTransactionError::Underpriced => RpcPoolError::Underpriced,
            InvalidPoolTransactionError::Other(err) => RpcPoolError::PoolTransactionError(err),
            InvalidPoolTransactionError::Eip4844(err) => RpcPoolError::Eip4844(err),
            InvalidPoolTransactionError::Overdraft => {
                RpcPoolError::Invalid(RpcInvalidTransactionError::InsufficientFunds)
            }
        }
    }
}

impl From<PoolError> for EthApiError {
    fn from(err: PoolError) -> Self {
        EthApiError::PoolError(RpcPoolError::from(err))
    }
}

/// Errors returned from a sign request.
#[derive(Debug, thiserror::Error)]
pub enum SignError {
    /// Error occured while trying to sign data.
    #[error("Could not sign")]
    CouldNotSign,
    /// Signer for requested account not found.
    #[error("Unknown account")]
    NoAccount,
    /// TypedData has invalid format.
    #[error("Given typed data is not valid")]
    TypedData,
    /// No chainid
    #[error("No chainid")]
    NoChainId,
}

/// Converts the evm [ExecutionResult] into a result where `Ok` variant is the output bytes if it is
/// [ExecutionResult::Success].
pub(crate) fn ensure_success(result: ExecutionResult) -> EthResult<Bytes> {
    match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data().into()),
        ExecutionResult::Revert { output, .. } => {
            Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into())
        }
        ExecutionResult::Halt { reason, gas_used } => {
            Err(RpcInvalidTransactionError::halt(reason, gas_used).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_error() {
        let err = EthApiError::ExecutionTimedOut(Duration::from_secs(10));
        assert_eq!(err.to_string(), "execution aborted (timeout = 10s)");
    }
}
