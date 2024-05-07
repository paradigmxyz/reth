use crate::{BlockNumHash, BlockNumber, B256};

/// Current status of the blockchain's head.
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// The block hash of the highest fully synced block.
    pub best_hash: B256,
    /// The block number of the highest fully synced block.
    pub best_number: BlockNumber,
}

impl From<ChainInfo> for BlockNumHash {
    fn from(value: ChainInfo) -> Self {
        BlockNumHash { number: value.best_number, hash: value.best_hash }
    }
}
