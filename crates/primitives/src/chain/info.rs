use crate::{BlockNumber, H256};

/// Current status of the blockchain's head.
#[derive(Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// Best block hash.
    pub best_hash: H256,
    /// Best block number.
    pub best_number: BlockNumber,
    /// Last block that was finalized.
    pub last_finalized: Option<BlockNumber>,
    /// Safe block
    pub safe_finalized: Option<BlockNumber>,
}
