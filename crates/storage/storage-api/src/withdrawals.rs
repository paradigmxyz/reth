use alloy_eips::{eip4895::Withdrawals, BlockHashOrNumber};
use alloy_primitives::BlockNumber;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

///  Client trait for fetching [`alloy_eips::eip4895::Withdrawal`] related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>>;

    /// Returns the withdrawals per block within the requested block range.
    ///
    /// If it's dealing with a pre-shangai block, the withdrawal element will be `None`, otherwise
    /// `Some`.
    fn withdrawals_by_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
        timestamps: &[(BlockNumber, u64)],
    ) -> ProviderResult<Vec<Option<Withdrawals>>>;
}
