use reth_primitives::{BlockHash, BlockNumber};

/// KV error type. They are using u32 to represent error code.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block number {block_number:?} does not exist in database")]
    BlockNumber { block_number: BlockNumber },
    #[error("Block hash {block_hash:?} does not exist in Headers table")]
    BlockHash { block_hash: BlockHash },
    #[error(
        "Block number {block_number:?} hash {block_hash:?} key does not exist in Bodies table"
    )]
    BlockNumberHash { block_number: BlockNumber, block_hash: BlockHash },
}
