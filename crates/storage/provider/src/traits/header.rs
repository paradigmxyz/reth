use auto_impl::auto_impl;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{BlockHash, BlockHashOrNumber, BlockNumber, Header, SealedHeader, U256};
use std::ops::RangeBounds;

/// Client trait for fetching `Header` related data.
#[auto_impl(&, Arc)]
pub trait HeaderProvider: Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> ProviderResult<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>>;

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> ProviderResult<Option<Header>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }

    /// Get total difficulty by block hash.
    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>>;

    /// Get total difficulty by block number.
    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>>;

    /// Get headers in range of block numbers
    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>>;

    /// Get a single sealed header by block number.
    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>>;

    /// Get headers in range of block numbers.
    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.sealed_headers_while(range, |_| true)
    }

    /// Get sealed headers while `predicate` returns `true` or the range is exhausted.
    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>>;
}
