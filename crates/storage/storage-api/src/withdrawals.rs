use alloy_eips::BlockHashOrNumber;
use reth_primitives::{Withdrawal, Withdrawals};
use reth_storage_errors::provider::ProviderResult;

///  Client trait for fetching [Withdrawal] related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>>;

    /// Get latest withdrawal from this block or earlier .
    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>>;
}
