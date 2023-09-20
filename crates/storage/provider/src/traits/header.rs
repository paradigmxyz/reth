use auto_impl::auto_impl;
use reth_interfaces::RethResult;
use reth_primitives::{BlockHash, BlockHashOrNumber, BlockNumber, Header, SealedHeader, U256};
use std::ops::RangeBounds;

/// Client trait for fetching `Header` related data.
#[auto_impl(&, Arc)]
pub trait HeaderProvider: Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> RethResult<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>>;

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> RethResult<Option<Header>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> RethResult<Option<Header>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }

    /// Get total difficulty by block hash.
    fn header_td(&self, hash: &BlockHash) -> RethResult<Option<U256>>;

    /// Get total difficulty by block number.
    fn header_td_by_number(&self, number: BlockNumber) -> RethResult<Option<U256>>;

    /// Get headers in range of block numbers
    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>>;

    /// Get headers in range of block numbers
    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>>;

    /// Get a single sealed header by block number
    fn sealed_header(&self, number: BlockNumber) -> RethResult<Option<SealedHeader>>;
}
