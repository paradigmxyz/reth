use reth_primitives::{BlockHash, BlockNumHash, Bloom, H256};
use thiserror::Error;

/// Transaction validation errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM { hash: H256, message: String },
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff { got: H256, expected: H256 },
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff { got: Box<Bloom>, expected: Box<Bloom> },
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        transaction_gas_limit: u64,
        block_available_gas: u64,
    },
    #[error("Block gas used {got} is different from expected gas used {expected}.")]
    BlockGasUsed { got: u64, expected: u64 },
    #[error("Block {hash:?} is pre merge")]
    BlockPreMerge { hash: H256 },
    #[error("Missing total difficulty")]
    MissingTotalDifficulty { hash: H256 },
}

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockExecutionError {
    #[error(transparent)]
    Validation(#[from] BlockValidationError),

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
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::CanonicalCommit { .. } | Self::CanonicalRevert { .. })
    }
}
