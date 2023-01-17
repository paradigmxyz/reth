use reth_primitives::{BlockHash, BlockNumber};

/// KV error type. They are using u32 to represent error code.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block number {block_number} does not exist in database")]
    BlockNumber { block_number: BlockNumber },
    #[error("Block hash {block_hash:?} does not exist in Headers table")]
    BlockHash { block_hash: BlockHash },
    #[error("Block body not exists #{block_number} ({block_hash:?})")]
    BlockBody { block_number: BlockNumber, block_hash: BlockHash },
    #[error("Block transition id does not exist for block #{block_number}")]
    BlockTransition { block_number: BlockNumber },

    #[error("Block number {block_number} from block hash #{block_hash} does not exist in canonical chain")]
    BlockCanonical { block_number: BlockNumber, block_hash: BlockHash },
    #[error("Block number {block_number} with hash #{received_hash:?} is not canonical block. Canonical block hash is #{expected_hash:?}")]
    NonCanonicalBlock {
        block_number: BlockNumber,
        expected_hash: BlockHash,
        received_hash: BlockHash,
    },
}
