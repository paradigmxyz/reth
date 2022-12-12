use reth_primitives::{BlockHash, BlockNumber};

/// KV error type. They are using u32 to represent error code.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block Number {block_number:?} does not exist in database")]
    BlockNumberNotExists { block_number: BlockNumber },
    #[error("Block tx cumulative number for hash {block_hash:?} does not exist in database")]
    BlockTxNumberNotExists { block_hash: BlockHash },
    #[error("Block hash {block_hash:?} does not exists in Headers table")]
    BlockHashNotExist { block_hash: BlockHash },
    #[error("Block body not exists #{block_number} {block_hash}")]
    BlockBodyNotExist { block_number: BlockNumber, block_hash: BlockHash },
}
