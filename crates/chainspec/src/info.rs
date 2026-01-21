use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};

/// Current status of the blockchain's head.
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// The block hash of the highest fully synced block.
    pub best_hash: B256,
    /// The block number of the highest fully synced block.
    pub best_number: BlockNumber,
    /// The block number of the earliest block we have available.
    ///
    /// This tracks the lowest block height still retained, useful for determining
    /// what data has expired (e.g., due to pruning).
    pub earliest_block: BlockNumber,
}

impl From<ChainInfo> for BlockNumHash {
    fn from(value: ChainInfo) -> Self {
        Self { number: value.best_number, hash: value.best_hash }
    }
}
