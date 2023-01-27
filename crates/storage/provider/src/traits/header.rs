use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{BlockHash, BlockHashOrNumber, Header, U256};

/// Client trait for fetching `Header` related data.
#[auto_impl(&)]
pub trait HeaderProvider: Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> Result<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>>;

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> Result<Option<Header>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(&self, hash_or_num: BlockHashOrNumber) -> Result<Option<Header>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }

    /// Get total difficulty by block hash.
    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>>;
}
