use auto_impl::auto_impl;
use reth_interfaces::RethResult;
use reth_primitives::{BlockHashOrNumber, BlockNumber, B256};

/// Client trait for fetching block hashes by number.
#[auto_impl(&, Arc, Box)]
pub trait BlockHashReader: Send + Sync {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: BlockNumber) -> RethResult<Option<B256>>;

    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn convert_block_hash(&self, hash_or_number: BlockHashOrNumber) -> RethResult<Option<B256>> {
        match hash_or_number {
            BlockHashOrNumber::Hash(hash) => Ok(Some(hash)),
            BlockHashOrNumber::Number(num) => self.block_hash(num),
        }
    }

    /// Get headers in range of block hashes or numbers
    ///
    /// Returns the available hashes of that range.
    ///
    /// Note: The range is `start..end`, so the expected result is `[start..end)`
    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber)
        -> RethResult<Vec<B256>>;
}
