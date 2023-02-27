use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{BlockNumber, H256};
use std::ops::RangeBounds;

/// Client trait for fetching block hashes by number.
#[auto_impl(&, Arc)]
pub trait BlockHashProvider: Send + Sync {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: u64) -> Result<Option<H256>>;

    /// Get headers in range of block hashes or numbers
    fn canonical_hashes_range<R: RangeBounds<BlockNumber>>(&self, range: R) -> Result<Vec<H256>>;
}
