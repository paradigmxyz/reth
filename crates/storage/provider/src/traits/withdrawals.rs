use reth_interfaces::Result;
use reth_primitives::{BlockId, Withdrawal};

///  Client trait for fetching [Withdrawal] related data.
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn withdrawals_by_block(&self, id: BlockId, timestamp: u64) -> Result<Option<Vec<Withdrawal>>>;
}
