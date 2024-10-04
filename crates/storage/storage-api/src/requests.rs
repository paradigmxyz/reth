use alloy_eips::BlockHashOrNumber;
use reth_primitives::Requests;
use reth_storage_errors::provider::ProviderResult;

/// Client trait for fetching EIP-7685 [Requests] for blocks.
#[auto_impl::auto_impl(&, Arc)]
pub trait RequestsProvider: Send + Sync {
    /// Get withdrawals by block id.
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Requests>>;
}
