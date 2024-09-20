use alloy_primitives::BlockNumber;
use reth_errors::ProviderResult;

/// Functionality to read the last known finalized block from the database.
pub trait FinalizedBlockReader: Send + Sync {
    /// Returns the last finalized block number.
    ///
    /// If no finalized block has been written yet, this returns `None`.
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>>;
}

/// Functionality to write the last known finalized block to the database.
pub trait FinalizedBlockWriter: Send + Sync {
    /// Saves the given finalized block number in the DB.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;
}
