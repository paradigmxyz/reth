use alloy_primitives::BlockNumber;
use reth_errors::ProviderResult;

/// Functionality to read the last known chain blocks from the database.
pub trait ChainStateBlockReader: Send + Sync {
    /// Returns the last finalized block number.
    ///
    /// If no finalized block has been written yet, this returns `None`.
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>>;
    /// Returns the last safe block number.
    ///
    /// If no safe block has been written yet, this returns `None`.
    fn last_safe_block_number(&self) -> ProviderResult<Option<BlockNumber>>;
}

/// Functionality to write the last known chain blocks to the database.
pub trait ChainStateBlockWriter: Send + Sync {
    /// Saves the given finalized block number in the DB.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;

    /// Saves the given safe block number in the DB.
    fn save_safe_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;
}
