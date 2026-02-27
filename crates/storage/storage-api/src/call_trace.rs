use alloy_primitives::{Address, BlockNumber};
use auto_impl::auto_impl;
use reth_storage_errors::provider::ProviderResult;

/// Reader for call trace indices (from/to address → block number bitmaps).
///
/// These indices map addresses to block numbers where they appeared as
/// transaction sender or recipient, enabling efficient `trace_filter` queries.
#[auto_impl(&, Arc, Box)]
pub trait CallTraceIndexReader: Send + Sync {
    /// Returns block numbers where the given address appeared as a transaction sender
    /// within the specified block range.
    fn call_trace_from_blocks(
        &self,
        address: Address,
        range: std::ops::RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockNumber>>;

    /// Returns block numbers where the given address appeared as a transaction recipient
    /// within the specified block range.
    fn call_trace_to_blocks(
        &self,
        address: Address,
        range: std::ops::RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockNumber>>;
}
