use crate::{GotExpectedBoxed, U256};

/// Represents error variants that can happen when trying to validate a
/// [Transaction](crate::Transaction)
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum InvalidTransactionError {
    /// The sender does not have enough funds to cover the transaction fees
    #[error(
        "sender does not have enough funds ({}) to cover transaction fees: {}", _0.got, _0.expected
    )]
    InsufficientFunds(GotExpectedBoxed<U256>),
    /// The nonce is lower than the account's nonce, or there is a nonce gap present.
    ///
    /// This is a consensus error.
    #[error("transaction nonce is not consistent")]
    NonceNotConsistent,
    /// The transaction is before Spurious Dragon and has a chain ID.
    #[error("transactions before Spurious Dragon should not have a chain ID")]
    OldLegacyChainId,
    /// The chain ID in the transaction does not match the current network configuration.
    #[error("transaction's chain ID does not match")]
    ChainIdMismatch,
    /// The transaction requires EIP-2930 which is not enabled currently.
    #[error("EIP-2930 transactions are disabled")]
    Eip2930Disabled,
    /// The transaction requires EIP-1559 which is not enabled currently.
    #[error("EIP-1559 transactions are disabled")]
    Eip1559Disabled,
    /// The transaction requires EIP-4844 which is not enabled currently.
    #[error("EIP-4844 transactions are disabled")]
    Eip4844Disabled,
    /// Thrown if a transaction is not supported in the current network configuration.
    #[error("transaction type not supported")]
    TxTypeNotSupported,
    /// The calculated gas of the transaction exceeds `u64::MAX`.
    #[error("gas overflow (maximum of u64)")]
    GasUintOverflow,
    /// The transaction is specified to use less gas than required to start the invocation.
    #[error("intrinsic gas too low")]
    GasTooLow,
    /// The transaction gas exceeds the limit
    #[error("intrinsic gas too high")]
    GasTooHigh,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    #[error("max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// Thrown post London if the transaction's fee is less than the base fee of the block.
    #[error("max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    #[error("transaction signer has bytecode set")]
    SignerAccountHasBytecode,
}
