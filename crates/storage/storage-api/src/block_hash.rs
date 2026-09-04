use alloc::vec::Vec;
use alloy_primitives::{BlockNumber, B256};
use reth_storage_errors::provider::ProviderResult;

/// Client trait for fetching block hashes by number.
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait BlockHashReader {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>>;

    /// Get headers in range of block hashes or numbers
    ///
    /// Returns the available hashes of that range.
    ///
    /// Note: The range is `start..end`, so the expected result is `[start..end)`
    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>>;
}
