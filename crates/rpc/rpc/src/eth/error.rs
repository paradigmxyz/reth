//! Implementation specific Errors for the `eth_` namespace.

use crate::result::{internal_rpc_err, invalid_params_rpc_err, rpc_err, rpc_error_with_code};
use alloy_sol_types::decode_revert_reason;
use jsonrpsee::{
    core::Error as RpcError,
    types::{error::CALL_EXECUTION_FAILED_CODE, ErrorObject},
};
use reth_interfaces::RethError;
use reth_primitives::{revm_primitives::InvalidHeader, Address, Bytes, U256};
use reth_revm::tracing::js::JsInspectorError;
use reth_rpc_types::{error::EthRpcErrorCode, request::TransactionInputError, BlockError};
use reth_transaction_pool::error::{
    Eip4844PoolTransactionError, InvalidPoolTransactionError, PoolError, PoolErrorKind,
    PoolTransactionError,
};
use revm::primitives::{EVMError, ExecutionResult, HaltReason, OutOfGasError};
use std::time::Duration;

/// Result alias
pub type EthResult<T> = Result<T, EthApiError>;

/// Errors that can occur when interacting with the `eth_` namespace
#[derive(Debug, thiserror::Error)]
pub enum EthApiError {
    /// When a raw transaction is empty
    #[error("empty transaction data")]
    EmptyRawTransactionData,
    /// When decoding a signed transaction fails
    #[error("failed to decode signed transaction")]
    FailedToDecodeSignedTransaction,
    /// When the transaction signature is invalid
    #[error("invalid transaction signature")]
    InvalidTransactionSignature,
    /// Errors related to the transaction pool
    #[error(transparent)]
    PoolError(RpcPoolError),
    /// When an unknown block number is encountered
    #[error("unknown block number")]
    UnknownBlockNumber,
    /// Thrown when querying for `finalized` or `safe` block before the merge transition is
    /// finalized, <https://github.com/ethereum/execution-apis/blob/6d17705a875e52c26826124c2a8a15ed542aeca2/src/schemas/block.yaml#L109>
    #[error("unknown block")]
    UnknownSafeOrFinalizedBlock,
    /// Thrown when an unknown block or transaction index is encountered
    #[error("unknown block or tx index")]
    UnknownBlockOrTxIndex,
    /// When an invalid block range is provided
    #[error("invalid block range")]
    InvalidBlockRange,
    /// An internal error where prevrandao is not set in the evm's environment
    #[error("prevrandao not in the EVM's environment after merge")]
    PrevrandaoNotSet,
    /// `excess_blob_gas` is not set for Cancun and above
    #[error("excess blob gas missing in the EVM's environment after Cancun")]
    ExcessBlobGasNotSet,
    /// Thrown when a call or transaction request (`eth_call`, `eth_estimateGas`,
    /// `eth_sendTransaction`) contains conflicting fields (legacy, EIP-1559)
    #[error("both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified")]
    ConflictingFeeFieldsInRequest,
    /// Errors related to invalid transactions
    #[error(transparent)]
    InvalidTransaction(#[from] RpcInvalidTransactionError),
    /// Thrown when constructing an RPC block from primitive block data fails
    #[error(transparent)]
    InvalidBlockData(#[from] BlockError),
    /// Thrown when an `AccountOverride` contains conflicting `state` and `stateDiff` fields
    #[error("account {0:?} has both 'state' and 'stateDiff'")]
    BothStateAndStateDiffInOverride(Address),
    /// Other internal error
    #[error(transparent)]
    Internal(RethError),
    /// Error related to signing
    #[error(transparent)]
    Signing(#[from] SignError),
    /// Thrown when a requested transaction is not found
    #[error("transaction not found")]
    TransactionNotFound,
    /// Some feature is unsupported
    #[error("unsupported")]
    Unsupported(&'static str),
    /// General purpose error for invalid params
    #[error("{0}")]
    InvalidParams(String),
    /// When the tracer config does not match the tracer
    #[error("invalid tracer config")]
    InvalidTracerConfig,
    /// When the percentile array is invalid
    #[error("invalid reward percentiles")]
    InvalidRewardPercentiles,
    /// Error thrown when a spawned blocking task failed to deliver an anticipated response.
    ///
    /// This only happens if the blocking task panics and is aborted before it can return a
    /// response back to the request handler.
    #[error("internal blocking task error")]
    InternalBlockingTaskError,
    /// Error thrown when a spawned blocking task failed to deliver an anticipated response
    #[error("internal eth error")]
    InternalEthError,
    /// Error thrown when a (tracing) call exceeds the configured timeout
    #[error("execution aborted (timeout = {0:?})")]
    ExecutionTimedOut(Duration),
    /// Internal Error thrown by the javascript tracer
    #[error("{0}")]
    InternalJsTracerError(String),
    #[error(transparent)]
    /// Call Input error when both `data` and `input` fields are set and not equal.
    TransactionInputError(#[from] TransactionInputError),
    /// Optimism related error
    #[error(transparent)]
    #[cfg(feature = "optimism")]
    Optimism(#[from] OptimismEthApiError),
    /// Evm generic purpose error.
    #[error("Revm error: {0}")]
    EvmCustom(String),
}

/// Eth Optimism Api Error
#[cfg(feature = "optimism")]
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
    /// Wrapper around a [hyper::Error].
    #[error(transparent)]
    HyperError(#[from] hyper::Error),
    /// Wrapper around an [reqwest::Error].
    #[error(transparent)]
    HttpError(#[from] reqwest::Error),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
    /// Thrown when calculating L1 gas fee
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
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
            EthApiError::ExcessBlobGasNotSet |
            EthApiError::InvalidBlockData(_) |
            EthApiError::Internal(_) |
            EthApiError::TransactionNotFound |
            EthApiError::EvmCustom(_) => internal_rpc_err(error.to_string()),
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
            err @ EthApiError::InternalBlockingTaskError => internal_rpc_err(err.to_string()),
            err @ EthApiError::InternalEthError => internal_rpc_err(err.to_string()),
            err @ EthApiError::TransactionInputError(_) => invalid_params_rpc_err(err.to_string()),
            #[cfg(feature = "optimism")]
            EthApiError::Optimism(err) => match err {
                OptimismEthApiError::HyperError(err) => internal_rpc_err(err.to_string()),
                OptimismEthApiError::HttpError(err) => internal_rpc_err(err.to_string()),
                OptimismEthApiError::InvalidSequencerTransaction |
                OptimismEthApiError::L1BlockFeeError |
                OptimismEthApiError::L1BlockGasError => internal_rpc_err(err.to_string()),
            },
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

impl From<RethError> for EthApiError {
    fn from(error: RethError) -> Self {
        match error {
            RethError::Provider(err) => err.into(),
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
            EVMError::Header(InvalidHeader::PrevrandaoNotSet) => EthApiError::PrevrandaoNotSet,
            EVMError::Header(InvalidHeader::ExcessBlobGasNotSet) => {
                EthApiError::ExcessBlobGasNotSet
            }
            EVMError::Database(err) => err.into(),
            EVMError::Custom(err) => EthApiError::EvmCustom(err),
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
    #[error("out of gas: gas required exceeds allowance: {0:?}")]
    BasicOutOfGas(U256),
    /// As BasicOutOfGas but thrown when gas exhausts during memory expansion.
    #[error("out of gas: gas exhausts during memory expansion: {0:?}")]
    MemoryOutOfGas(U256),
    /// As BasicOutOfGas but thrown when gas exhausts during precompiled contract execution.
    #[error("out of gas: gas exhausts during precompiled contract execution: {0:?}")]
    PrecompileOutOfGas(U256),
    /// revm's Type cast error, U256 casts down to a u64 with overflow
    #[error("out of gas: revm's Type cast error, U256 casts down to a u64 with overflow {0:?}")]
    InvalidOperandOutOfGas(U256),
    /// Thrown if executing a transaction failed during estimate/call
    #[error("{0}")]
    Revert(RevertError),
    /// Unspecific EVM halt error.
    #[error("EVM error {0:?}")]
    EvmHalt(HaltReason),
    /// Invalid chain id set for the transaction.
    #[error("invalid chain ID")]
    InvalidChainId,
    /// The transaction is before Spurious Dragon and has a chain ID
    #[error("transactions before Spurious Dragon should not have a chain ID")]
    OldLegacyChainId,
    /// The transitions is before Berlin and has access list
    #[error("transactions before Berlin should not have access list")]
    AccessListNotSupported,
    /// `max_fee_per_blob_gas` is not supported for blocks before the Cancun hardfork.
    #[error("max_fee_per_blob_gas is not supported for blocks before the Cancun hardfork")]
    MaxFeePerBlobGasNotSupported,
    /// `blob_hashes`/`blob_versioned_hashes` is not supported for blocks before the Cancun
    /// hardfork.
    #[error("blob_versioned_hashes is not supported for blocks before the Cancun hardfork")]
    BlobVersionedHashesNotSupported,
    /// Block `blob_base_fee` is greater than tx-specified `max_fee_per_blob_gas` after Cancun.
    #[error("max fee per blob gas less than block blob gas fee")]
    BlobFeeCapTooLow,
    /// Blob transaction has a versioned hash with an invalid blob
    #[error("blob hash version mismatch")]
    BlobHashVersionMismatch,
    /// Blob transaction has no versioned hashes
    #[error("blob transaction missing blob hashes")]
    BlobTransactionMissingBlobHashes,
    /// Blob transaction has too many blobs
    #[error("blob transaction exceeds max blobs per block")]
    TooManyBlobs,
    /// Blob transaction is a create transaction
    #[error("blob transaction is a create transaction")]
    BlobTransactionIsCreate,
    /// Optimism related error
    #[error(transparent)]
    #[cfg(feature = "optimism")]
    Optimism(#[from] OptimismInvalidTransactionError),
}

/// Optimism specific invalid transaction errors
#[cfg(feature = "optimism")]
#[derive(thiserror::Error, Debug)]
pub enum OptimismInvalidTransactionError {
    /// A deposit transaction was submitted as a system transaction post-regolith.
    #[error("no system transactions allowed after regolith")]
    DepositSystemTxPostRegolith,
    /// A deposit transaction halted post-regolith
    #[error("deposit transaction halted after regolith")]
    HaltedDepositPostRegolith,
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
    pub(crate) fn halt(reason: HaltReason, gas_limit: u64) -> Self {
        match reason {
            HaltReason::OutOfGas(err) => RpcInvalidTransactionError::out_of_gas(err, gas_limit),
            HaltReason::NonceOverflow => RpcInvalidTransactionError::NonceMaxValue,
            err => RpcInvalidTransactionError::EvmHalt(err),
        }
    }

    /// Converts the out of gas error
    pub(crate) fn out_of_gas(reason: OutOfGasError, gas_limit: u64) -> Self {
        let gas_limit = U256::from(gas_limit);
        match reason {
            OutOfGasError::Basic => RpcInvalidTransactionError::BasicOutOfGas(gas_limit),
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
            InvalidTransaction::PriorityFeeGreaterThanMaxFee => {
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
            InvalidTransaction::LackOfFundForMaxFee { .. } => {
                RpcInvalidTransactionError::InsufficientFunds
            }
            InvalidTransaction::OverflowPaymentInTransaction => {
                RpcInvalidTransactionError::GasUintOverflow
            }
            InvalidTransaction::NonceOverflowInTransaction => {
                RpcInvalidTransactionError::NonceMaxValue
            }
            InvalidTransaction::CreateInitCodeSizeLimit => {
                RpcInvalidTransactionError::MaxInitCodeSizeExceeded
            }
            InvalidTransaction::NonceTooHigh { .. } => RpcInvalidTransactionError::NonceTooHigh,
            InvalidTransaction::NonceTooLow { .. } => RpcInvalidTransactionError::NonceTooLow,
            InvalidTransaction::AccessListNotSupported => {
                RpcInvalidTransactionError::AccessListNotSupported
            }
            InvalidTransaction::MaxFeePerBlobGasNotSupported => {
                RpcInvalidTransactionError::MaxFeePerBlobGasNotSupported
            }
            InvalidTransaction::BlobVersionedHashesNotSupported => {
                RpcInvalidTransactionError::BlobVersionedHashesNotSupported
            }
            InvalidTransaction::BlobGasPriceGreaterThanMax => {
                RpcInvalidTransactionError::BlobFeeCapTooLow
            }
            InvalidTransaction::EmptyBlobs => {
                RpcInvalidTransactionError::BlobTransactionMissingBlobHashes
            }
            InvalidTransaction::BlobVersionNotSupported => {
                RpcInvalidTransactionError::BlobHashVersionMismatch
            }
            InvalidTransaction::TooManyBlobs => RpcInvalidTransactionError::TooManyBlobs,
            InvalidTransaction::BlobCreateTransaction => {
                RpcInvalidTransactionError::BlobTransactionIsCreate
            }
            #[cfg(feature = "optimism")]
            InvalidTransaction::DepositSystemTxPostRegolith => {
                RpcInvalidTransactionError::Optimism(
                    OptimismInvalidTransactionError::DepositSystemTxPostRegolith,
                )
            }
            #[cfg(feature = "optimism")]
            InvalidTransaction::HaltedDepositPostRegolith => RpcInvalidTransactionError::Optimism(
                OptimismInvalidTransactionError::HaltedDepositPostRegolith,
            ),
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
    output: Option<Bytes>,
}

// === impl RevertError ==

impl RevertError {
    /// Wraps the output bytes
    ///
    /// Note: this is intended to wrap an revm output
    pub fn new(output: Bytes) -> Self {
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
        if let Some(reason) = self.output.as_ref().and_then(|bytes| decode_revert_reason(bytes)) {
            write!(f, ": {reason}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RevertError {}

/// A helper error type that's mainly used to mirror `geth` Txpool's error messages
#[derive(Debug, thiserror::Error)]
pub enum RpcPoolError {
    /// When the transaction is already known
    #[error("already known")]
    AlreadyKnown,
    /// When the sender is invalid
    #[error("invalid sender")]
    InvalidSender,
    /// When the transaction is underpriced
    #[error("transaction underpriced")]
    Underpriced,
    /// When the transaction pool is full
    #[error("txpool is full")]
    TxPoolOverflow,
    /// When the replacement transaction is underpriced
    #[error("replacement transaction underpriced")]
    ReplaceUnderpriced,
    /// When the transaction exceeds the block gas limit
    #[error("exceeds block gas limit")]
    ExceedsGasLimit,
    /// When a negative value is encountered
    #[error("negative value")]
    NegativeValue,
    /// When oversized data is encountered
    #[error("oversized data")]
    OversizedData,
    /// When the max initcode size is exceeded
    #[error("max initcode size exceeded")]
    ExceedsMaxInitCodeSize,
    /// Errors related to invalid transactions
    #[error(transparent)]
    Invalid(#[from] RpcInvalidTransactionError),
    /// Custom pool error
    #[error(transparent)]
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
    /// Other unspecified error
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
        match err.kind {
            PoolErrorKind::ReplacementUnderpriced => RpcPoolError::ReplaceUnderpriced,
            PoolErrorKind::FeeCapBelowMinimumProtocolFeeCap(_) => RpcPoolError::Underpriced,
            PoolErrorKind::SpammerExceededCapacity(_) => RpcPoolError::TxPoolOverflow,
            PoolErrorKind::DiscardedOnInsert => RpcPoolError::TxPoolOverflow,
            PoolErrorKind::InvalidTransaction(err) => err.into(),
            PoolErrorKind::Other(err) => RpcPoolError::Other(err),
            PoolErrorKind::AlreadyImported => RpcPoolError::AlreadyKnown,
            PoolErrorKind::ExistingConflictingTransactionType(_, _) => {
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
            InvalidPoolTransactionError::IntrinsicGasTooLow => {
                RpcPoolError::Invalid(RpcInvalidTransactionError::GasTooLow)
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
    #[error("could not sign")]
    CouldNotSign,
    /// Signer for requested account not found.
    #[error("unknown account")]
    NoAccount,
    /// TypedData has invalid format.
    #[error("given typed data is not valid")]
    InvalidTypedData,
    /// Invalid transaction request in `sign_transaction`.
    #[error("invalid transaction request")]
    InvalidTransactionRequest,
    /// No chain ID was given.
    #[error("no chainid")]
    NoChainId,
}

/// Converts the evm [ExecutionResult] into a result where `Ok` variant is the output bytes if it is
/// [ExecutionResult::Success].
pub(crate) fn ensure_success(result: ExecutionResult) -> EthResult<Bytes> {
    match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
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
