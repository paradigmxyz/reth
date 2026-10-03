use alloc::vec::Vec;
use alloy_primitives::{BlockNumber, TxNumber};
use core::ops::RangeInclusive;
use reth_db_models::StoredBlockBodyIndices;
use reth_storage_errors::provider::{ProviderError, ProviderResult};

///  Client trait for fetching block body indices related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockBodyIndicesProvider: Send {
    /// Returns the block body indices with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>>;

    /// Returns the block body indices within the requested range matching number from storage.
    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>>;

    /// Returns the first transaction number after `block`, including when its body indices were
    /// never written, e.g. at a snap sync pivot.
    fn next_tx_num_after_block(&self, block: BlockNumber) -> ProviderResult<TxNumber> {
        if let Some(indices) = self.block_body_indices(block)? {
            return Ok(indices.next_tx_num())
        }
        if let Some(next) = block.checked_add(1) &&
            let Some(indices) = self.block_body_indices(next)?
        {
            return Ok(indices.first_tx_num())
        }
        Err(ProviderError::BlockBodyIndicesNotFound(block))
    }
}
