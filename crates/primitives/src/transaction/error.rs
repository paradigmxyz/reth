use crate::{GotExpectedBoxed, U256};

/// Represents error variants that can happen when trying to validate a
/// [Transaction](crate::Transaction)
#[derive(Debug, Clone, Eq, PartialEq, derive_more::Display)]
pub enum InvalidTransactionError {
    /// The sender does not have enough funds to cover the transaction fees
    #[display(
        fmt = "sender does not have enough funds ({}) to cover transaction fees: {}", _0.got, _0.expected
    )]
    InsufficientFunds(GotExpectedBoxed<U256>),
    /// The nonce is lower than the account's nonce, or there is a nonce gap present.
    ///
    /// This is a consensus error.
    #[display(fmt = "transaction nonce is not consistent")]
    NonceNotConsistent,
    /// The transaction is before Spurious Dragon and has a chain ID.
    #[display(fmt = "transactions before Spurious Dragon should not have a chain ID")]
    OldLegacyChainId,
    /// The chain ID in the transaction does not match the current network configuration.
    #[display(fmt = "transaction's chain ID does not match")]
    ChainIdMismatch,
    /// The transaction requires EIP-2930 which is not enabled currently.
    #[display(fmt = "EIP-2930 transactions are disabled")]
    Eip2930Disabled,
    /// The transaction requires EIP-1559 which is not enabled currently.
    #[display(fmt = "EIP-1559 transactions are disabled")]
    Eip1559Disabled,
    /// The transaction requires EIP-4844 which is not enabled currently.
    #[display(fmt = "EIP-4844 transactions are disabled")]
    Eip4844Disabled,
    /// The transaction requires EIP-7702 which is not enabled currently.
    #[display(fmt = "EIP-7702 transactions are disabled")]
    Eip7702Disabled,
    /// Thrown if a transaction is not supported in the current network configuration.
    #[display(fmt = "transaction type not supported")]
    TxTypeNotSupported,
    /// The calculated gas of the transaction exceeds `u64::MAX`.
    #[display(fmt = "gas overflow (maximum of u64)")]
    GasUintOverflow,
    /// The transaction is specified to use less gas than required to start the invocation.
    #[display(fmt = "intrinsic gas too low")]
    GasTooLow,
    /// The transaction gas exceeds the limit
    #[display(fmt = "intrinsic gas too high")]
    GasTooHigh,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    #[display(fmt = "max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// Thrown post London if the transaction's fee is less than the base fee of the block.
    #[display(fmt = "max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    #[display(fmt = "transaction signer has bytecode set")]
    SignerAccountHasBytecode,
}

#[cfg(feature = "std")]
impl std::error::Error for InvalidTransactionError {}

/// Represents error variants that can happen when trying to convert a transaction to
/// [`PooledTransactionsElement`](crate::PooledTransactionsElement)
#[derive(Debug, Clone, Eq, PartialEq, derive_more::Display)]
pub enum TransactionConversionError {
    /// This error variant is used when a transaction cannot be converted into a
    /// [`PooledTransactionsElement`](crate::PooledTransactionsElement) because it is not supported
    /// for P2P network.
    #[display(fmt = "Transaction is not supported for p2p")]
    UnsupportedForP2P,
}

#[cfg(feature = "std")]
impl std::error::Error for TransactionConversionError {}

/// Represents error variants than can happen when trying to convert a
/// [`TransactionSignedEcRecovered`](crate::TransactionSignedEcRecovered) transaction.
#[derive(Debug, Clone, Eq, PartialEq, derive_more::Display)]
pub enum TryFromRecoveredTransactionError {
    /// Thrown if the transaction type is unsupported.
    #[display(fmt = "Unsupported transaction type: {_0}")]
    UnsupportedTransactionType(u8),
    /// This error variant is used when a blob sidecar is missing.
    #[display(fmt = "Blob sidecar missing for an EIP-4844 transaction")]
    BlobSidecarMissing,
}

#[cfg(feature = "std")]
impl std::error::Error for TryFromRecoveredTransactionError {}
