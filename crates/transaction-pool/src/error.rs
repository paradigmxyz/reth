//! Transaction pool errors

use reth_primitives::{Address, TxHash};

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("[{0:?}]: insufficient gas price to replace existing transaction.")]
    ReplacementUnderpriced(TxHash),
    /// Encountered a transaction that was already added into the poll
    #[error("[{0:?}] Transaction feeCap {1} below chain minimum.")]
    ProtocolFeeCapTooLow(TxHash, u128),
    /// Thrown when the number of unique transactions of a sender exceeded the slot capacity.
    #[error("{0:?} identified as spammer. Transaction {1:?} rejected.")]
    SpammerExceededCapacity(Address, TxHash),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("[{0:?}] Transaction discarded outright due to pool size constraints.")]
    DiscardedOnInsert(TxHash),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("[{0:?}] Transaction's gas limit {1} exceeds block's gas limit {2}.")]
    TxExceedsGasLimit(TxHash, u64, u64),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the max_init_code_size.
    #[error("[{0:?}] Transaction's size {1} exceeds max_init_code_size {2}.")]
    TxExceedsMaxInitCodeSize(TxHash, usize, usize),
    /// Thrown if the transaction contains an invalid signature
    #[error("[{0:?}]: Invalid sender")]
    AccountNotFound(TxHash),
    /// Thrown if the input data of a transaction is greater
    /// than some meaningful limit a user might use. This is not a consensus error
    /// making the transaction invalid, rather a DOS protection.
    #[error("[{0:?}]: Input data too large")]
    OversizedData(TxHash, usize, usize),


    #[error("Account does not have enough funds ({available_funds:?}) to cover transaction max fee: {max_fee:?}.")]
    InsufficientFunds { max_fee: u128, available_funds: U256 },
    #[error("Transaction nonce is not consistent.")]
    TransactionNonceNotConsistent,
    #[error("Transaction chain_id does not match.")]
    TransactionChainId,
    #[error("Transaction max fee is less them block base fee.")]
    TransactionMaxFeeLessThenBaseFee,
    #[error("Eip2930 transaction is enabled after berlin hardfork.")]
    TransactionEip2930Disabled,
    #[error("Eip2930 transaction is enabled after london hardfork.")]
    TransactionEip1559Disabled,
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

// === impl PoolError ===

impl PoolError {
    /// Returns the hash of the transaction that resulted in this error.
    pub fn hash(&self) -> &TxHash {
        match self {
            PoolError::ReplacementUnderpriced(hash) => hash,
            PoolError::ProtocolFeeCapTooLow(hash, _) => hash,
            PoolError::SpammerExceededCapacity(_, hash) => hash,
            PoolError::DiscardedOnInsert(hash) => hash,
            PoolError::TxExceedsGasLimit(hash, _, _) => hash,
            PoolError::TxExceedsMaxInitCodeSize(hash, _, _) => hash,
            PoolError::AccountNotFound(hash) => hash,
            PoolError::OversizedData(hash, _, _) => hash,
        }
    }
}
