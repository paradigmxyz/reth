use crate::U256;

/// Represents error variants that can happen when trying to validate a
/// [Transaction](crate::Transaction)
#[allow(missing_docs)]
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum InvalidTransactionError {
    #[error("Transaction eip1559 priority fee is more then max fee.")]
    PriorityFeeMoreThenMaxFee,
    #[error("Account does not have enough funds ({available_funds:?}) to cover transaction max fee: {max_fee:?}.")]
    InsufficientFunds { max_fee: u128, available_funds: U256 },
    #[error("Transaction nonce is not consistent.")]
    NonceNotConsistent,
    #[error("Old legacy transaction before Spurious Dragon should not have chain_id.")]
    OldLegacyChainId,
    #[error("Transaction chain_id does not match.")]
    ChainIdMismatch,
    #[error("Transaction max fee is less them block base fee.")]
    MaxFeeLessThenBaseFee,
    #[error("Eip2930 transaction is enabled after berlin hardfork.")]
    Eip2930Disabled,
    #[error("Eip2930 transaction is enabled after london hardfork.")]
    Eip1559Disabled,
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
    /// Thrown post London if the transaction's fee is less than the base fee of the block
    #[error("max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown if the sender of a transaction is a contract.
    #[error("Transaction signer has bytecode set.")]
    SignerAccountHasBytecode,
}
