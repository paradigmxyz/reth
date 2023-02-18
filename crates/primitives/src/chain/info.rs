use crate::{rpc::BlockNumber as RpcBlockNumber, BlockNumber, H256};

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
    pub fn convert_block_number(&self, number: RpcBlockNumber) -> Option<u64> {
        match number {
            RpcBlockNumber::Finalized => self.last_finalized,
            RpcBlockNumber::Safe => self.safe_finalized,
            RpcBlockNumber::Earliest => Some(0),
            RpcBlockNumber::Number(num) => Some(num.as_u64()),
            RpcBlockNumber::Pending => None,
            RpcBlockNumber::Latest => Some(self.best_number),
        }
    }
}
