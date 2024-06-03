use reth_errors::ProviderResult;
use reth_primitives::BlockNumber;

/// Functionality to read the last known finalized block from the database.
pub trait FinalizedBlockReader: Send + Sync {
    /// Fetches and returns the latest finalized block number.
    fn fetch_latest_finalized_block_number(&self) -> ProviderResult<BlockNumber>;
}

/// Functionality to write the last known finalized block to the database.
pub trait FinalizedBlockWriter: Send + Sync {
    /// Saves the given finalized block number in the DB.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;
}
