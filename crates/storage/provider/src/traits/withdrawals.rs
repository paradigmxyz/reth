use reth_interfaces::RethResult;
use reth_primitives::{BlockHashOrNumber, Withdrawal};

///  Client trait for fetching [Withdrawal] related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> RethResult<Option<Vec<Withdrawal>>>;

    /// Get latest withdrawal from this block or earlier .
    fn latest_withdrawal(&self) -> RethResult<Option<Withdrawal>>;
}
