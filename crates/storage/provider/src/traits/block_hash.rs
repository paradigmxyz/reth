use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{H256, U256};

/// Client trait for fetching block hashes by number.
#[auto_impl(&)]
pub trait BlockHashProvider: Send + Sync {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: U256) -> Result<Option<H256>>;
}
