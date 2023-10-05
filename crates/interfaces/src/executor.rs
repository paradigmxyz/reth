use reth_primitives::{BlockNumHash, Bloom, PruneSegmentError, B256};
use thiserror::Error;

/// Transaction validation errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// Error message
        message: String,
    },
    /// Error when recovering the sender for a transaction
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    /// Error when incrementing balance in post execution
    #[error("Incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when receipt root doesn't match expected value
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff {
        /// The actual receipt root
        got: B256,
        /// The expected receipt root
        expected: B256,
    },
    /// Error when header bloom filter doesn't match expected value
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff {
        /// The actual bloom filter
        got: Box<Bloom>,
        /// The expected bloom filter
        expected: Box<Bloom>,
    },
    /// Error when transaction gas limit exceeds available block gas
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error when block gas used doesn't match expected value
    #[error("Block gas used {got} is different from expected gas used {expected}.\nGas spent by each transaction: {gas_spent_by_tx:?}\n")]
    BlockGasUsed {
        /// The actual gas used
        got: u64,
        /// The expected gas used
        expected: u64,
        /// Gas spent by each transaction
        gas_spent_by_tx: Vec<(u64, u64)>,
    },
    /// Error for pre-merge block
    #[error("Block {hash:?} is pre merge")]
    BlockPreMerge {
        /// The hash of the block
        hash: B256,
    },
    #[error("Missing total difficulty for block {hash:?}")]
    MissingTotalDifficulty { hash: B256 },
    /// Error for EIP-4788 when parent beacon block root is missing
    #[error("EIP-4788 Parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    #[error("The parent beacon block root is not zero for Cancun genesis block")]
    CancunGenesisParentBeaconBlockRootNotZero,
}

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping `BlockValidationError`
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Pruning error, transparently wrapping `PruneSegmentError`
    #[error(transparent)]
    Pruning(#[from] PruneSegmentError),
    /// Error representing a provider error
    #[error("Provider error")]
    ProviderError,
    /// Transaction error on revert with inner details
    #[error("Transaction error on revert: {inner:?}")]
    CanonicalRevert {
        /// The inner error message
        inner: String,
    },
    /// Transaction error on commit with inner details
    #[error("Transaction error on commit: {inner:?}")]
    CanonicalCommit {
        /// The inner error message
        inner: String,
    },
    /// Error when appending chain on fork is not possible
    #[error(
        "Appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect {
        /// The tip of the current chain
        chain_tip: BlockNumHash,
        /// The fork on the other chain
        other_chain_fork: BlockNumHash,
    },
    /// Only used for TestExecutor
    ///
    /// Note: this is not feature gated for convenience.
    #[error("Execution unavailable for tests")]
    UnavailableForTest,
}

impl BlockExecutionError {
    /// Returns `true` if the error is fatal.
    ///
    /// This represents an unrecoverable database related error.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::CanonicalCommit { .. } | Self::CanonicalRevert { .. })
    }
}
