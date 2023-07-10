use reth_interfaces::Result;
use reth_primitives::{BlockHashOrNumber, Withdrawal};

///  Client trait for fetching [Withdrawal] related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> Result<Option<Vec<Withdrawal>>>;

    /// Get latest withdrawal from this block or earlier .
    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>>;
}
