use alloy_primitives::BlockNumber;
use reth_db_api::models::StoredBlockBodyIndices;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::SealedBlockWithSenders;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};

/// An enum that represents the storage location for a piece of data.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StorageLocation {
    /// Write only to static files.
    StaticFiles,
    /// Write only to the database.
    Database,
    /// Write to both the database and static files.
    Both,
}

impl StorageLocation {
    /// Returns true if the storage location includes static files.
    pub const fn static_files(&self) -> bool {
        matches!(self, Self::StaticFiles | Self::Both)
    }

    /// Returns true if the storage location includes the database.
    pub const fn database(&self) -> bool {
        matches!(self, Self::Database | Self::Both)
    }
}

/// BlockExecution Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockExecutionWriter: BlockWriter + Send + Sync {
    /// Take all of the blocks above the provided number and their execution result
    ///
    /// The passed block number will stay in the database.
    fn take_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_transactions_from: StorageLocation,
    ) -> ProviderResult<Chain>;

    /// Remove all of the blocks above the provided number and their execution result
    ///
    /// The passed block number will stay in the database.
    fn remove_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_transactions_from: StorageLocation,
    ) -> ProviderResult<()>;
}

/// This just receives state, or [`ExecutionOutcome`], from the provider
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StateReader: Send + Sync {
    /// Get the [`ExecutionOutcome`] for the given block
    fn get_state(&self, block: BlockNumber) -> ProviderResult<Option<ExecutionOutcome>>;
}

/// Block Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockWriter: Send + Sync {
    /// The body this writer can write.
    type Block: reth_primitives_traits::Block;

    /// Insert full block and make it canonical. Parent tx num and transition id is taken from
    /// parent block in database.
    ///
    /// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
    /// transition in the block.
    ///
    /// Accepts [`StorageLocation`] value which specifies where transactions and headers should be
    /// written.
    fn insert_block(
        &self,
        block: SealedBlockWithSenders<Self::Block>,
        write_to: StorageLocation,
    ) -> ProviderResult<StoredBlockBodyIndices>;

    /// Appends a batch of block bodies extending the canonical chain. This is invoked during
    /// `Bodies` stage and does not write to `TransactionHashNumbers` and `TransactionSenders`
    /// tables which are populated on later stages.
    ///
    /// Bodies are passed as [`Option`]s, if body is `None` the corresponding block is empty.
    fn append_block_bodies(
        &self,
        bodies: Vec<(BlockNumber, Option<<Self::Block as reth_primitives_traits::Block>::Body>)>,
        write_transactions_to: StorageLocation,
    ) -> ProviderResult<()>;

    /// Removes all blocks above the given block number from the database.
    ///
    /// Note: This does not remove state or execution data.
    fn remove_blocks_above(
        &self,
        block: BlockNumber,
        remove_transactions_from: StorageLocation,
    ) -> ProviderResult<()>;

    /// Removes all block bodies above the given block number from the database.
    fn remove_bodies_above(
        &self,
        block: BlockNumber,
        remove_transactions_from: StorageLocation,
    ) -> ProviderResult<()>;

    /// Appends a batch of sealed blocks to the blockchain, including sender information, and
    /// updates the post-state.
    ///
    /// Inserts the blocks into the database and updates the state with
    /// provided `BundleState`.
    ///
    /// # Parameters
    ///
    /// - `blocks`: Vector of `SealedBlockWithSenders` instances to append.
    /// - `state`: Post-state information to update after appending.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if any operation fails.
    fn append_blocks_with_state(
        &self,
        blocks: Vec<SealedBlockWithSenders<Self::Block>>,
        execution_outcome: ExecutionOutcome,
        hashed_state: HashedPostStateSorted,
        trie_updates: TrieUpdates,
    ) -> ProviderResult<()>;
}
