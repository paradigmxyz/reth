use std::ops::RangeInclusive;

use reth_interfaces::Result;
use reth_primitives::{Address, BlockNumber, IntegerList, H256};

/// Client trait for fetching [reth_primitives::Log] index data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogIndexProvider: Send + Sync {
    /// Get the list of block numbers within a block range where a given address emitted a log.
    ///
    /// Returns `None` of no block numbers matched the address.
    fn log_address_indices(
        &self,
        address: Address,
        block_range: RangeInclusive<BlockNumber>,
    ) -> Result<Option<IntegerList>>;

    /// Get the list of block numbers within a block range where a given topic was emitted in a log.
    ///
    /// Returns `None` of no block numbers matched the topic.
    fn log_topic_indices(
        &self,
        topic: H256,
        block_range: RangeInclusive<BlockNumber>,
    ) -> Result<Option<IntegerList>>;
}
