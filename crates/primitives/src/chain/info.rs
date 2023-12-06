use crate::{BlockNumber, B256};

/// Current status of the blockchain's head.
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// The block hash of the highest fully synced block.
    pub best_hash: B256,
    /// The block number of the highest fully synced block.
    pub best_number: BlockNumber,
}
