use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, Bloom, H256};
use thiserror::Error;

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM { hash: H256, message: String },
    #[error("Verification failed.")]
    VerificationFailed,
    #[error("Fatal internal error")]
    ExecutionFatalError,
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    #[error("Receipt cumulative gas used {got:?} is different from expected {expected:?}")]
    ReceiptCumulativeGasUsedDiff { got: u64, expected: u64 },
    #[error("Receipt log count {got:?} is different from expected {expected:?}.")]
    ReceiptLogCountDiff { got: usize, expected: usize },
    #[error("Receipt log is different.")]
    ReceiptLogDiff,
    #[error("Receipt log is different.")]
    ExecutionSuccessDiff { got: bool, expected: bool },
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff { got: H256, expected: H256 },
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff { got: Box<Bloom>, expected: Box<Bloom> },
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThenAvailableBlockGas {
        transaction_gas_limit: u64,
        block_available_gas: u64,
    },
    #[error("Block gas used {got} is different from expected gas used {expected}.")]
    BlockGasUsed { got: u64, expected: u64 },
    #[error("Provider error")]
    ProviderError,
    #[error("BlockChainId can't be found in BlockchainTree with internal index {chain_id}")]
    BlockSideChainIdConsistency { chain_id: u64 },
    #[error(
        "Appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect { chain_tip: BlockNumHash, other_chain_fork: BlockNumHash },
    #[error("Canonical chain header #{block_hash} can't be found ")]
    CanonicalChain { block_hash: BlockHash },
    #[error("Can't insert #{block_number} {block_hash} as last finalized block number is {last_finalized}")]
    PendingBlockIsFinalized {
        block_hash: BlockHash,
        block_number: BlockNumber,
        last_finalized: BlockNumber,
    },
    #[error("Block number #{block_number} not found in blockchain tree chain")]
    BlockNumberNotFoundInChain { block_number: BlockNumber },
    #[error("Block hash {block_hash} not found in blockchain tree chain")]
    BlockHashNotFoundInChain { block_hash: BlockHash },
    #[error("Transaction error on revert: {inner:?}")]
    CanonicalRevert { inner: String },
    #[error("Transaction error on commit: {inner:?}")]
    CanonicalCommit { inner: String },
    #[error("Transaction error on pipeline status update: {inner:?}")]
    PipelineStatusUpdate { inner: String },
    #[error("Block {hash:?} is pre merge")]
    BlockPreMerge { hash: H256 },
    #[error("Missing total difficulty")]
    MissingTotalDifficulty { hash: H256 },
}
