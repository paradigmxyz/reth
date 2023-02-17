use crate::{BlockNumber, H256};

/// Current status of the blockchain's head.
#[derive(Default, Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// The block hash of the highest fully synced block.
    pub best_hash: H256,
    /// The block number of the highest fully synced block.
    pub best_number: BlockNumber,
    /// Last block that was finalized.
    pub last_finalized: Option<BlockNumber>,
    /// Safe block
    pub safe_finalized: Option<BlockNumber>,
}
