use crate::{provider::ProviderError, trie::StateRootError};
use reth_primitives::{
    revm_primitives::EVMError, BlockNumHash, Bloom, GotExpected, GotExpectedBoxed,
    PruneSegmentError, B256,
};
use thiserror::Error;

/// Transaction validation errors
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    #[error("EVM reported invalid transaction ({hash}): {error}")]
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        #[source]
        error: Box<EVMError<ProviderError>>,
    },
    /// Error when recovering the sender for a transaction
    #[error("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// Error when incrementing balance in post execution
    #[error("incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when receipt root doesn't match expected value
    #[error("receipt root mismatch: {0}")]
    ReceiptRootDiff(GotExpectedBoxed<B256>),
    /// Error when header bloom filter doesn't match expected value
    #[error("header bloom filter mismatch: {0}")]
    BloomLogDiff(GotExpectedBoxed<Bloom>),
    /// Error when the state root does not match the expected value.
    #[error(transparent)]
    StateRoot(#[from] StateRootError),
    /// Error when transaction gas limit exceeds available block gas
    #[error("transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error when block gas used doesn't match expected value
    #[error("block gas used mismatch: {gas}; gas spent by each transaction: {gas_spent_by_tx:?}")]
    BlockGasUsed {
        /// The gas diff.
        gas: GotExpected<u64>,
        /// Gas spent by each transaction
        gas_spent_by_tx: Vec<(u64, u64)>,
    },
    /// Error for pre-merge block
    #[error("block {hash} is pre merge")]
    BlockPreMerge {
        /// The hash of the block
        hash: B256,
    },
    /// Error for missing total difficulty
    #[error("missing total difficulty for block {hash}")]
    MissingTotalDifficulty {
        /// The hash of the block
        hash: B256,
    },
    /// Error for EIP-4788 when parent beacon block root is missing
    #[error("EIP-4788 parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    #[error("the parent beacon block root is not zero for Cancun genesis block: {parent_beacon_block_root}")]
    CancunGenesisParentBeaconBlockRootNotZero {
        /// The beacon block root
        parent_beacon_block_root: B256,
    },
    /// EVM error during beacon root contract call
    #[error("failed to apply beacon root contract call at {parent_beacon_block_root}: {message}")]
    BeaconRootContractCall {
        /// The beacon block root
        parent_beacon_block_root: Box<B256>,
        /// The error message.
        message: String,
    },
}

/// BlockExecutor Errors
#[derive(Error, Debug)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping `BlockValidationError`
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Pruning error, transparently wrapping `PruneSegmentError`
    #[error(transparent)]
    Pruning(#[from] PruneSegmentError),
    /// Transaction error on revert with inner details
    #[error("transaction error on revert: {inner}")]
    CanonicalRevert {
        /// The inner error message
        inner: String,
    },
    /// Transaction error on commit with inner details
    #[error("transaction error on commit: {inner}")]
    CanonicalCommit {
        /// The inner error message
        inner: String,
    },
    /// Error when appending chain on fork is not possible
    #[error(
        "appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect {
        /// The tip of the current chain
        chain_tip: Box<BlockNumHash>,
        /// The fork on the other chain
        other_chain_fork: Box<BlockNumHash>,
    },
    /// Only used for TestExecutor
    ///
    /// Note: this is not feature gated for convenience.
    #[error("execution unavailable for tests")]
    UnavailableForTest,
    /// Error when fetching latest block state.
    #[error(transparent)]
    LatestBlock(#[from] ProviderError),
    /// Optimism Block Executor Errors
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl BlockExecutionError {
    /// Create a new `BlockExecutionError::Other` variant.
    pub fn other<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Returns the inner `BlockValidationError` if the error is a validation error.
    pub const fn as_validation(&self) -> Option<&BlockValidationError> {
        match self {
            Self::Validation(err) => Some(err),
            _ => None,
        }
    }

    /// Returns `true` if the error is fatal.
    ///
    /// This represents an unrecoverable database related error.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::CanonicalCommit { .. } | Self::CanonicalRevert { .. })
    }

    /// Returns `true` if the error is a state root error.
    pub fn is_state_root_error(&self) -> bool {
        matches!(self, Self::Validation(BlockValidationError::StateRoot(_)))
    }
}
