use crate::{GotExpectedBoxed, U256};
use core::fmt;

/// Represents error variants that can happen when trying to validate a
/// [Transaction](crate::Transaction)
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InvalidTransactionError {
    /// The sender does not have enough funds to cover the transaction fees
    // #[error(
    //     "sender does not have enough funds ({}) to cover transaction fees: {}", _0.got, _0.expected
    // )]
    InsufficientFunds(GotExpectedBoxed<U256>),
    /// The nonce is lower than the account's nonce, or there is a nonce gap present.
    ///
    /// This is a consensus error.
    NonceNotConsistent,
    /// The transaction is before Spurious Dragon and has a chain ID.
    OldLegacyChainId,
    /// The chain ID in the transaction does not match the current network configuration.
    ChainIdMismatch,
    /// The transaction requires EIP-2930 which is not enabled currently.
    Eip2930Disabled,
    /// The transaction requires EIP-1559 which is not enabled currently.
    Eip1559Disabled,
    /// The transaction requires EIP-4844 which is not enabled currently.
    Eip4844Disabled,
    /// The transaction requires EIP-7702 which is not enabled currently.
    Eip7702Disabled,
    /// Thrown if a transaction is not supported in the current network configuration.
    TxTypeNotSupported,
    /// The calculated gas of the transaction exceeds `u64::MAX`.
    GasUintOverflow,
    /// The transaction is specified to use less gas than required to start the invocation.
    GasTooLow,
    /// The transaction gas exceeds the limit
    GasTooHigh,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    TipAboveFeeCap,
    /// Thrown post London if the transaction's fee is less than the base fee of the block.
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    SignerAccountHasBytecode,
}

#[cfg(feature = "std")]
impl std::error::Error for InvalidTransactionError {}

impl fmt::Display for InvalidTransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientFunds(values) => {
                f.write_fmt(format_args!("sender does not have enough funds ({}) to cover transaction fees: {}", values.got, values.expected))
            },
            Self::NonceNotConsistent => f.write_str("transaction nonce is not consistent"),
            Self::OldLegacyChainId => f.write_str("transactions before Spurious Dragon should not have a chain ID"),
            Self::ChainIdMismatch => f.write_str("transaction's chain ID does not match"),
            Self::Eip2930Disabled => f.write_str("EIP-2930 transactions are disabled"),
            Self::Eip1559Disabled => f.write_str("EIP-1559 transactions are disabled"),
            Self::Eip4844Disabled => f.write_str("EIP-4844 transactions are disabled"),
            Self::Eip7702Disabled => f.write_str("EIP-7702 transactions are disabled"),
            Self::TxTypeNotSupported => f.write_str("transaction type not supported"),
            Self::GasUintOverflow => f.write_str("gas overflow (maximum of u64)"),
            Self::GasTooLow => f.write_str("intrinsic gas too low"),
            Self::GasTooHigh => f.write_str("intrinsic gas too high"),
            Self::TipAboveFeeCap => f.write_str("max priority fee per gas higher than max fee per gas"),
            Self::FeeCapTooLow => f.write_str("max fee per gas less than block base fee"),
            Self::SignerAccountHasBytecode => f.write_str("transaction signer has bytecode set"),
        }
    }
}

/// Represents error variants that can happen when trying to convert a transaction to
/// [`PooledTransactionsElement`](crate::PooledTransactionsElement)
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionConversionError {
    /// This error variant is used when a transaction cannot be converted into a
    /// [`PooledTransactionsElement`](crate::PooledTransactionsElement) because it is not supported
    /// for P2P network.
    UnsupportedForP2P,
}

#[cfg(feature = "std")]
impl std::error::Error for TransactionConversionError {}

impl fmt::Display for TransactionConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedForP2P => f.write_str("Transaction is not supported for p2p"),
        }
    }
}

/// Represents error variants than can happen when trying to convert a
/// [`TransactionSignedEcRecovered`](crate::TransactionSignedEcRecovered) transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TryFromRecoveredTransactionError {
    /// Thrown if the transaction type is unsupported.
    // #[error("Unsupported transaction type: {0}")]
    UnsupportedTransactionType(u8),
    /// Thrown if an EIP-4844 transaction's blob sidecar is missing
    // #[error("Blob sidecar missing for an EIP-4844 transaction")]
    BlobSidecarMissing,
}

#[cfg(feature = "std")]
impl std::error::Error for TryFromRecoveredTransactionError {}

impl fmt::Display for TryFromRecoveredTransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTransactionType(tx_type) => {
                f.write_fmt(format_args!("Unsupported transaction type: {0}", tx_type))
            },
            Self::BlobSidecarMissing => f.write_str("Blob sidecar missig for an EIP-4844 transaction"),
        }
    }
}