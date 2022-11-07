use async_trait::async_trait;
use reth_primitives::{BlockHash, BlockNumber, SealedHeader, H256};
use tokio::sync::watch::Receiver;

/// Re-export forkchoice state
pub use reth_rpc_types::engine::ForkchoiceState;

/// Consensus is a protocol that chooses canonical chain.
/// We are checking validity of block header here.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Send + Sync {
    /// Get a receiver for the fork choice state
    fn fork_choice_state(&self) -> Receiver<ForkchoiceState>;

    /// Validate if header is correct and follows consensus specification
    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error>;
}

/// Consensus Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block used gas ({gas_used:?}) is greater then gas limit ({gas_limit:?})")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommner hash ({got:?}) is different then expected: ({expected:?})")]
    BodyOmmnersHashDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different then expected: ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different then expected: ({expected:?})")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
    #[error("Block with [hash:{hash:?},number: {number:}] is already known")]
    BlockKnown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent [hash:{hash:?}] is not known")]
    ParentUnknown { hash: BlockHash },
    #[error("Block number {block_number:?} is missmatch with parent block number {parent_block_number:?}")]
    ParentBlockNumberMissmatch { parent_block_number: BlockNumber, block_number: BlockNumber },
    #[error(
        "Block timestamp {timestamp:?} is in past in comparison with parent timestamp {parent_timestamp:?}"
    )]
    TimestampIsInPast { parent_timestamp: u64, timestamp: u64 },
    #[error("Block timestamp {timestamp:?} is in future in comparison of our clock time {present_timestamp:?}")]
    TimestampIsInFuture { timestamp: u64, present_timestamp: u64 },
    // TODO make better error msg :)
    #[error("Child gas_limit {child_gas_limit:?} max increase is {parent_gas_limit}/1024")]
    GasLimitInvalidIncrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Child gas_limit {child_gas_limit:?} max decrease is {parent_gas_limit}/1024")]
    GasLimitInvalidDecrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Base fee missing")]
    BaseFeeMissing,
    #[error("Block base fee ({got:?}) is different then expected: ({expected:?})")]
    BaseFeeDiff { expected: u64, got: u64 },
}
