use crate::{BlockNumber, BlockNumberOrTag, H256};

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

impl ChainInfo {
    /// Attempts to convert a [BlockNumber](crate::rpc::BlockNumber) enum to a numeric value
    pub fn convert_block_number(&self, number: BlockNumberOrTag) -> Option<u64> {
        match number {
            BlockNumberOrTag::Finalized => self.last_finalized,
            BlockNumberOrTag::Safe => self.safe_finalized,
            BlockNumberOrTag::Earliest => Some(0),
            BlockNumberOrTag::Number(num) => Some(num),
            BlockNumberOrTag::Pending => None,
            BlockNumberOrTag::Latest => Some(self.best_number),
        }
    }
}
