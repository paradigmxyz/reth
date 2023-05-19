use crate::U256;

/// Represents error variants that can happen when trying to validate a
/// [Transaction](crate::Transaction)
#[allow(missing_docs)]
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum InvalidTransactionError {
    /// The sender does not have enough funds to cover the transaction fees
    #[error("Sender does not have enough funds ({available_funds:?}) to cover transaction fees: {cost:?}.")]
    InsufficientFunds { cost: U256, available_funds: U256 },
    /// The nonce is lower than the account's nonce, or there is a nonce gap present.
    #[error("Transaction nonce is not consistent.")]
    NonceNotConsistent,
    /// The transaction is before Spurious Dragon and has a chain ID
    #[error("Transactions before Spurious Dragon should not have a chain ID.")]
    OldLegacyChainId,
    /// The chain ID in the transaction does not match the current network configuration.
    #[error("Transaction's chain ID does not match.")]
    ChainIdMismatch,
    /// The transaction requires EIP-2930 which is not enabled currently.
    #[error("EIP-2930 transactions are not valid before Berlin.")]
    Eip2930Disabled,
    /// The transaction requires EIP-1559 which is not enabled currently.
    #[error("EIP-1559 transactions are not valid before London.")]
    Eip1559Disabled,
    /// Thrown if a transaction is not supported in the current network configuration.
    #[error("Transaction type not supported")]
    TxTypeNotSupported,
    /// The calculated gas of the transaction exceeds `u64::MAX`.
    #[error("Gas overflow (maximum of u64)")]
    GasUintOverflow,
    /// The transaction is specified to use less gas than required to start the
    /// invocation.
    #[error("Intrinsic gas too low")]
    GasTooLow,
    /// The transaction gas exceeds the limit
    #[error("Intrinsic gas too high")]
    GasTooHigh,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    #[error("Max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// Thrown post London if the transaction's fee is less than the base fee of the block
    #[error("Max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    #[error("Transaction signer has bytecode set.")]
    SignerAccountHasBytecode,
}
