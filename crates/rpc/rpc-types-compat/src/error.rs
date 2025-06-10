//! A generic RPC transaction error definition.

use alloy_primitives::{Bytes, U256};
use alloy_rpc_types_eth::error::EthRpcErrorCode;
use alloy_sol_types::{ContractError, RevertReason};
use reth_evm::revm::context_interface::result::{HaltReason, InvalidTransaction, OutOfGasError};
use reth_primitives_traits::transaction::error::InvalidTransactionError;

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
    #[error("nonce too low: next nonce {state}, tx nonce {tx}")]
    NonceTooLow {
        /// The nonce of the transaction.
        tx: u64,
        /// The current state of the nonce in the local chain.
        state: u64,
    },
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
    /// Represents the inability to cover max fee + value (account balance too low).
    #[error("insufficient funds for gas * price + value: have {balance} want {cost}")]
    InsufficientFunds {
        /// Transaction cost.
        cost: U256,
        /// Current balance of transaction sender.
        balance: U256,
    },
    /// This is similar to [`Self::InsufficientFunds`] but with a different error message and
    /// exists for compatibility reasons.
    ///
    /// This error is used in `eth_estimateCall` when the highest available gas limit, capped with
    /// the allowance of the caller is too low: [`Self::GasTooLow`].
    #[error("gas required exceeds allowance ({gas_limit})")]
    GasRequiredExceedsAllowance {
        /// The gas limit the transaction was executed with.
        gas_limit: u64,
    },
    /// Thrown when calculating gas usage
    #[error("gas uint64 overflow")]
    GasUintOverflow,
    /// Thrown if the transaction is specified to use less gas than required to start the
    /// invocation.
    #[error("intrinsic gas too low")]
    GasTooLow,
    /// Thrown if the transaction gas exceeds the limit
    #[error("intrinsic gas too high")]
    GasTooHigh,
    /// Thrown if the transaction gas limit exceeds the maximum
    #[error("gas limit too high")]
    GasLimitTooHigh,
    /// Thrown if a transaction is not supported in the current network configuration.
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
    #[error("sender is not an EOA")]
    SenderNoEOA,
    /// Gas limit was exceeded during execution.
    /// Contains the gas limit.
    #[error("out of gas: gas required exceeds: {0}")]
    BasicOutOfGas(u64),
    /// Gas limit was exceeded during memory expansion.
    /// Contains the gas limit.
    #[error("out of gas: gas exhausted during memory expansion: {0}")]
    MemoryOutOfGas(u64),
    /// Gas limit was exceeded during precompile execution.
    /// Contains the gas limit.
    #[error("out of gas: gas exhausted during precompiled contract execution: {0}")]
    PrecompileOutOfGas(u64),
    /// An operand to an opcode was invalid or out of range.
    /// Contains the gas limit.
    #[error("out of gas: invalid operand to an opcode: {0}")]
    InvalidOperandOutOfGas(u64),
    /// Thrown if executing a transaction failed during estimate/call
    #[error(transparent)]
    Revert(RevertError),
    /// Unspecific EVM halt error.
    #[error("EVM error: {0:?}")]
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
    #[error("blob transaction exceeds max blobs per block; got {have}")]
    TooManyBlobs {
        /// The number of blobs in the transaction.
        have: usize,
    },
    /// Blob transaction is a create transaction
    #[error("blob transaction is a create transaction")]
    BlobTransactionIsCreate,
    /// EOF crate should have `to` address
    #[error("EOF crate should have `to` address")]
    EofCrateShouldHaveToAddress,
    /// EIP-7702 is not enabled.
    #[error("EIP-7702 authorization list not supported")]
    AuthorizationListNotSupported,
    /// EIP-7702 transaction has invalid fields set.
    #[error("EIP-7702 authorization list has invalid fields")]
    AuthorizationListInvalidFields,
    /// Any other error
    #[error("{0}")]
    Other(Box<dyn ToRpcError>),
}

impl RpcInvalidTransactionError {
    /// crates a new [`RpcInvalidTransactionError::Other`] variant.
    pub fn other<E: ToRpcError>(err: E) -> Self {
        Self::Other(Box::new(err))
    }
}

impl RpcInvalidTransactionError {
    /// Returns the rpc error code for this error.
    pub const fn error_code(&self) -> i32 {
        match self {
            Self::InvalidChainId |
            Self::GasTooLow |
            Self::GasTooHigh |
            Self::GasRequiredExceedsAllowance { .. } => EthRpcErrorCode::InvalidInput.code(),
            Self::Revert(_) => EthRpcErrorCode::ExecutionError.code(),
            _ => EthRpcErrorCode::TransactionRejected.code(),
        }
    }

    /// Converts the halt error
    ///
    /// Takes the configured gas limit of the transaction which is attached to the error
    pub const fn halt(reason: HaltReason, gas_limit: u64) -> Self {
        match reason {
            HaltReason::OutOfGas(err) => Self::out_of_gas(err, gas_limit),
            HaltReason::NonceOverflow => Self::NonceMaxValue,
            err => Self::EvmHalt(err),
        }
    }

    /// Converts the out of gas error
    pub const fn out_of_gas(reason: OutOfGasError, gas_limit: u64) -> Self {
        match reason {
            OutOfGasError::Basic | OutOfGasError::ReentrancySentry => {
                Self::BasicOutOfGas(gas_limit)
            }
            OutOfGasError::Memory | OutOfGasError::MemoryLimit => Self::MemoryOutOfGas(gas_limit),
            OutOfGasError::Precompile => Self::PrecompileOutOfGas(gas_limit),
            OutOfGasError::InvalidOperand => Self::InvalidOperandOutOfGas(gas_limit),
        }
    }
}

impl From<RpcInvalidTransactionError> for jsonrpsee_types::error::ErrorObject<'static> {
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
            RpcInvalidTransactionError::Other(err) => err.to_rpc_error(),
            err => rpc_err(err.error_code(), err.to_string(), None),
        }
    }
}

impl From<InvalidTransaction> for RpcInvalidTransactionError {
    fn from(err: InvalidTransaction) -> Self {
        match err {
            InvalidTransaction::InvalidChainId => Self::InvalidChainId,
            InvalidTransaction::PriorityFeeGreaterThanMaxFee => Self::TipAboveFeeCap,
            InvalidTransaction::GasPriceLessThanBasefee => Self::FeeCapTooLow,
            InvalidTransaction::CallerGasLimitMoreThanBlock => {
                // tx.gas > block.gas_limit
                Self::GasTooHigh
            }
            InvalidTransaction::CallGasCostMoreThanGasLimit { .. } => {
                // tx.gas < cost
                Self::GasTooLow
            }
            InvalidTransaction::GasFloorMoreThanGasLimit { .. } => {
                // Post prague EIP-7623 tx floor calldata gas cost > tx.gas_limit
                // where floor gas is the minimum amount of gas that will be spent
                // In other words, the tx's gas limit is lower that the minimum gas requirements of
                // the tx's calldata
                Self::GasTooLow
            }
            InvalidTransaction::RejectCallerWithCode => Self::SenderNoEOA,
            InvalidTransaction::LackOfFundForMaxFee { fee, balance } => {
                Self::InsufficientFunds { cost: *fee, balance: *balance }
            }
            InvalidTransaction::OverflowPaymentInTransaction => Self::GasUintOverflow,
            InvalidTransaction::NonceOverflowInTransaction => Self::NonceMaxValue,
            InvalidTransaction::CreateInitCodeSizeLimit => Self::MaxInitCodeSizeExceeded,
            InvalidTransaction::NonceTooHigh { .. } => Self::NonceTooHigh,
            InvalidTransaction::NonceTooLow { tx, state } => Self::NonceTooLow { tx, state },
            InvalidTransaction::AccessListNotSupported => Self::AccessListNotSupported,
            InvalidTransaction::MaxFeePerBlobGasNotSupported => Self::MaxFeePerBlobGasNotSupported,
            InvalidTransaction::BlobVersionedHashesNotSupported => {
                Self::BlobVersionedHashesNotSupported
            }
            InvalidTransaction::BlobGasPriceGreaterThanMax => Self::BlobFeeCapTooLow,
            InvalidTransaction::EmptyBlobs => Self::BlobTransactionMissingBlobHashes,
            InvalidTransaction::BlobVersionNotSupported => Self::BlobHashVersionMismatch,
            InvalidTransaction::TooManyBlobs { have, .. } => Self::TooManyBlobs { have },
            InvalidTransaction::BlobCreateTransaction => Self::BlobTransactionIsCreate,
            InvalidTransaction::EofCreateShouldHaveToAddress => Self::EofCrateShouldHaveToAddress,
            InvalidTransaction::AuthorizationListNotSupported => {
                Self::AuthorizationListNotSupported
            }
            InvalidTransaction::AuthorizationListInvalidFields |
            InvalidTransaction::EmptyAuthorizationList => Self::AuthorizationListInvalidFields,
            InvalidTransaction::Eip2930NotSupported |
            InvalidTransaction::Eip1559NotSupported |
            InvalidTransaction::Eip4844NotSupported |
            InvalidTransaction::Eip7702NotSupported |
            InvalidTransaction::Eip7873NotSupported => Self::TxTypeNotSupported,
            InvalidTransaction::Eip7873MissingTarget => {
                Self::other(internal_rpc_err(err.to_string()))
            }
        }
    }
}

impl From<InvalidTransactionError> for RpcInvalidTransactionError {
    fn from(err: InvalidTransactionError) -> Self {
        use InvalidTransactionError;
        // This conversion is used to convert any transaction errors that could occur inside the
        // txpool (e.g. `eth_sendRawTransaction`) to their corresponding RPC
        match err {
            InvalidTransactionError::InsufficientFunds(res) => {
                Self::InsufficientFunds { cost: res.expected, balance: res.got }
            }
            InvalidTransactionError::NonceNotConsistent { tx, state } => {
                Self::NonceTooLow { tx, state }
            }
            InvalidTransactionError::OldLegacyChainId => {
                // Note: this should be unreachable since Spurious Dragon now enabled
                Self::OldLegacyChainId
            }
            InvalidTransactionError::ChainIdMismatch => Self::InvalidChainId,
            InvalidTransactionError::Eip2930Disabled |
            InvalidTransactionError::Eip1559Disabled |
            InvalidTransactionError::Eip4844Disabled |
            InvalidTransactionError::Eip7702Disabled |
            InvalidTransactionError::TxTypeNotSupported => Self::TxTypeNotSupported,
            InvalidTransactionError::GasUintOverflow => Self::GasUintOverflow,
            InvalidTransactionError::GasTooLow => Self::GasTooLow,
            InvalidTransactionError::GasTooHigh => Self::GasTooHigh,
            InvalidTransactionError::TipAboveFeeCap => Self::TipAboveFeeCap,
            InvalidTransactionError::FeeCapTooLow => Self::FeeCapTooLow,
            InvalidTransactionError::SignerAccountHasBytecode => Self::SenderNoEOA,
            InvalidTransactionError::GasLimitTooHigh => Self::GasLimitTooHigh,
        }
    }
}

fn internal_rpc_err(msg: impl Into<String>) -> jsonrpsee_types::error::ErrorObject<'static> {
    rpc_err(jsonrpsee_types::error::INTERNAL_ERROR_CODE, msg, None)
}

fn rpc_err(
    code: i32,
    msg: impl Into<String>,
    data: Option<&[u8]>,
) -> jsonrpsee_types::error::ErrorObject<'static> {
    jsonrpsee_types::error::ErrorObject::owned(
        code,
        msg.into(),
        data.map(|data| {
            jsonrpsee_core::to_json_raw_value(&alloy_primitives::hex::encode_prefixed(data))
                .expect("serializing String can't fail")
        }),
    )
}

/// Represents a reverted transaction and its output data.
///
/// Displays "execution reverted(: reason)?" if the reason is a string.
#[derive(Debug, Clone, thiserror::Error)]
pub struct RevertError {
    /// The transaction output data
    ///
    /// Note: this is `None` if output was empty
    output: Option<Bytes>,
}

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

    /// Returns error code to return for this error.
    pub const fn error_code(&self) -> i32 {
        EthRpcErrorCode::ExecutionError.code()
    }
}

impl std::fmt::Display for RevertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("execution reverted")?;
        if let Some(reason) = self.output.as_ref().and_then(|out| RevertReason::decode(out)) {
            let error = reason.to_string();
            let mut error = error.as_str();
            if matches!(reason, RevertReason::ContractError(ContractError::Revert(_))) {
                // we strip redundant `revert: ` prefix from the revert reason
                error = error.trim_start_matches("revert: ");
            }
            write!(f, ": {error}")?;
        }
        Ok(())
    }
}

/// A trait to convert an error to an RPC error.
pub trait ToRpcError: core::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn to_rpc_error(&self) -> jsonrpsee_types::ErrorObject<'static>;
}

impl ToRpcError for jsonrpsee_types::ErrorObject<'static> {
    fn to_rpc_error(&self) -> jsonrpsee_types::ErrorObject<'static> {
        self.clone()
    }
}
