//! Implementation specific Errors for the `eth_` namespace.

use crate::result::{internal_rpc_err, rpc_err};
use jsonrpsee::{core::Error as RpcError, types::error::INVALID_PARAMS_CODE};
use reth_primitives::{constants::SELECTOR_LEN, Address, U128, U256};
use reth_rpc_types::{error::EthRpcErrorCode, BlockError};
use reth_transaction_pool::error::{InvalidPoolTransactionError, PoolError};
use revm::primitives::{EVMError, Halt, OutOfGasError};

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
    /// Thrown when a [AccountOverride](reth_rpc_types::state::AccountOverride) contains
    /// conflicting `state` and `stateDiff` fields
    #[error("account {0:?} has both 'state' and 'stateDiff'")]
    BothStateAndStateDiffInOverride(Address),
    /// Other internal error
    #[error(transparent)]
    Internal(#[from] reth_interfaces::Error),
    /// Error related to signing
    #[error(transparent)]
    Signing(#[from] SignError),
}

impl From<EthApiError> for RpcError {
    fn from(error: EthApiError) -> Self {
        match error {
            EthApiError::FailedToDecodeSignedTransaction |
            EthApiError::InvalidTransactionSignature |
            EthApiError::EmptyRawTransactionData |
            EthApiError::UnknownBlockNumber |
            EthApiError::InvalidBlockRange |
            EthApiError::ConflictingRequestGasPrice { .. } |
            EthApiError::ConflictingRequestGasPriceAndTipSet { .. } |
            EthApiError::RequestLegacyGasPriceAndTipSet { .. } |
            EthApiError::Signing(_) |
            EthApiError::BothStateAndStateDiffInOverride(_) => {
                rpc_err(INVALID_PARAMS_CODE, error.to_string(), None)
            }
            EthApiError::InvalidTransaction(err) => err.into(),
            EthApiError::PoolError(_) |
            EthApiError::PrevrandaoNotSet |
            EthApiError::InvalidBlockData(_) |
            EthApiError::Internal(_) => internal_rpc_err(error.to_string()),
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
}

impl InvalidTransactionError {
    /// Returns the rpc error code for this error.
    fn error_code(&self) -> i32 {
        match self {
            InvalidTransactionError::InvalidChainId |
            InvalidTransactionError::GasTooLow |
            InvalidTransactionError::GasTooHigh => EthRpcErrorCode::InvalidInput.code(),
            InvalidTransactionError::Revert(_) => EthRpcErrorCode::ExecutionError.code(),
            _ => EthRpcErrorCode::TransactionRejected.code(),
        }
    }

    /// Converts the out of gas error
    pub(crate) fn out_of_gas(reason: OutOfGasError, gas_limit: U256) -> Self {
        match reason {
            OutOfGasError::BasicOutOfGas => InvalidTransactionError::BasicOutOfGas(gas_limit),
            OutOfGasError::Memory => InvalidTransactionError::MemoryOutOfGas(gas_limit),
            OutOfGasError::Precompile => InvalidTransactionError::PrecompileOutOfGas(gas_limit),
            OutOfGasError::InvalidOperand => {
                InvalidTransactionError::InvalidOperandOutOfGas(gas_limit)
            }
            OutOfGasError::MemoryLimit => InvalidTransactionError::MemoryOutOfGas(gas_limit),
        }
    }
}

impl From<InvalidTransactionError> for RpcError {
    fn from(err: InvalidTransactionError) -> Self {
        match err {
            InvalidTransactionError::Revert(revert) => {
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

impl From<revm::primitives::InvalidTransaction> for InvalidTransactionError {
    fn from(err: revm::primitives::InvalidTransaction) -> Self {
        use revm::primitives::InvalidTransaction;
        match err {
            InvalidTransaction::InvalidChainId => InvalidTransactionError::InvalidChainId,
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
    GasLimit,
    #[error("negative value")]
    NegativeValue,
    #[error("oversized data")]
    OversizedData,
    #[error(transparent)]
    Invalid(#[from] InvalidPoolTransactionError),
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl From<PoolError> for RpcPoolError {
    fn from(err: PoolError) -> RpcPoolError {
        match err {
            PoolError::ReplacementUnderpriced(_) => RpcPoolError::ReplaceUnderpriced,
            PoolError::ProtocolFeeCapTooLow(_, _) => RpcPoolError::Underpriced,
            PoolError::SpammerExceededCapacity(_, _) => RpcPoolError::TxPoolOverflow,
            PoolError::DiscardedOnInsert(_) => RpcPoolError::TxPoolOverflow,
            PoolError::InvalidTransaction(_, err) => err.into(),
            PoolError::Other(_, err) => RpcPoolError::Other(err),
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
}

/// Returns the revert reason from the `revm::TransactOut` data, if it's an abi encoded String.
///
/// **Note:** it's assumed the `out` buffer starts with the call's signature
pub(crate) fn decode_revert_reason(out: impl AsRef<[u8]>) -> Option<String> {
    use ethers_core::abi::AbiDecode;
    let out = out.as_ref();
    if out.len() < SELECTOR_LEN {
        return None
    }
    String::decode(&out[SELECTOR_LEN..]).ok()
}
