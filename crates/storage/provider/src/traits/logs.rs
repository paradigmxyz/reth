use reth_interfaces::provider::ProviderResult;
use reth_primitives::{Address, BlockNumber, IntegerList, LogAddressIndex, LogTopicIndex, B256};
use std::ops::RangeInclusive;

/// Client trait for fetching log history data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogHistoryReader: Send + Sync {
    /// Get the list of block numbers within a block range where a given address emitted a log.
    ///
    /// Returns `None` of no block numbers matched the address.
    fn log_address_index(
        &self,
        address: Address,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<IntegerList>>;

    /// Get the list of block numbers within a block range where a given topic was emitted in a log.
    ///
    /// Returns `None` of no block numbers matched the topic.
    fn log_topic_index(
        &self,
        topic: B256,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<IntegerList>>;
}

/// Client trait for writing log history data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogHistoryWriter: Send + Sync {
    /// Insert log address index into the database. Used inside LogHistoryIndex stage.
    fn insert_log_address_history_index(&self, index: LogAddressIndex) -> ProviderResult<()>;

    /// Insert log topic index into the database. Used inside LogHistoryIndex stage.
    fn insert_log_topic_history_index(&self, index: LogTopicIndex) -> ProviderResult<()>;

    /// Write log address and topic history indexes.
    fn write_log_history_indexes(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;

    /// Unwind log address and topic history indexes.
    fn unwind_log_history_indexes(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;
}
