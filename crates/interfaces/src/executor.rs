use reth_primitives::{BlockHash, BlockNumHash, Bloom, PrunePartError, H256};
use thiserror::Error;

/// Transaction validation errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM { hash: H256, message: String },
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    #[error("Incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff { got: H256, expected: H256 },
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff { got: Box<Bloom>, expected: Box<Bloom> },
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        transaction_gas_limit: u64,
        block_available_gas: u64,
    },
    #[error("Block gas used {got} is different from expected gas used {expected}.\nGas spent by each transaction: {gas_spent_by_tx:?}\n")]
    BlockGasUsed { got: u64, expected: u64, gas_spent_by_tx: Vec<(u64, u64)> },
    #[error("Block {hash:?} is pre merge")]
    BlockPreMerge { hash: H256 },
    #[error("Missing total difficulty")]
    MissingTotalDifficulty { hash: H256 },
    #[error("EIP-4788 Parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    #[error("The parent beacon block root is not zero for Cancun genesis block")]
    CancunGenesisParentBeaconBlockRootNotZero,
}

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockExecutionError {
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    #[error(transparent)]
    Pruning(#[from] PrunePartError),

    // === misc provider error ===
    #[error("Provider error")]
    ProviderError,

    // === transaction errors ===
    #[error("Transaction error on revert: {inner:?}")]
    CanonicalRevert { inner: String },
    #[error("Transaction error on commit: {inner:?}")]
    CanonicalCommit { inner: String },

    // === tree errors ===
    // TODO(mattsse): move this to tree error
    #[error("Block hash {block_hash} not found in blockchain tree chain")]
    BlockHashNotFoundInChain { block_hash: BlockHash },
    #[error(
        "Appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect { chain_tip: BlockNumHash, other_chain_fork: BlockNumHash },

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
