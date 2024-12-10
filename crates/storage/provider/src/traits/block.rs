use alloy_primitives::BlockNumber;
use reth_db_api::models::StoredBlockBodyIndices;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_node_types::NodePrimitives;
use reth_primitives::SealedBlockWithSenders;
use reth_storage_api::{NodePrimitivesProvider, StorageLocation};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};

/// `BlockExecution` Writer
pub trait BlockExecutionWriter:
    NodePrimitivesProvider<Primitives: NodePrimitives<Block = Self::Block>> + BlockWriter + Send + Sync
{
    /// Take all of the blocks above the provided number and their execution result
    ///
    /// The passed block number will stay in the database.
    ///
    /// Accepts [`StorageLocation`] specifying from where should transactions and receipts be
    /// removed.
    fn take_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
    ) -> ProviderResult<Chain<Self::Primitives>>;

    /// Remove all of the blocks above the provided number and their execution result
    ///
    /// The passed block number will stay in the database.
    ///
    /// Accepts [`StorageLocation`] specifying from where should transactions and receipts be
    /// removed.
    fn remove_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
    ) -> ProviderResult<()>;
}

impl<T: BlockExecutionWriter> BlockExecutionWriter for &T {
    fn take_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
    ) -> ProviderResult<Chain<Self::Primitives>> {
        (*self).take_block_and_execution_above(block, remove_from)
    }

    fn remove_block_and_execution_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
    ) -> ProviderResult<()> {
        (*self).remove_block_and_execution_above(block, remove_from)
    }
}

/// This just receives state, or [`ExecutionOutcome`], from the provider
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StateReader: Send + Sync {
    /// Receipt type in [`ExecutionOutcome`].
    type Receipt: Send + Sync;

    /// Get the [`ExecutionOutcome`] for the given block
    fn get_state(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>>;
}

/// Block Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockWriter: Send + Sync {
    /// The body this writer can write.
    type Block: reth_primitives_traits::Block;
    /// The receipt type for [`ExecutionOutcome`].
    type Receipt: Send + Sync;

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
        write_to: StorageLocation,
    ) -> ProviderResult<()>;

    /// Removes all blocks above the given block number from the database.
    ///
    /// Note: This does not remove state or execution data.
    fn remove_blocks_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
    ) -> ProviderResult<()>;

    /// Removes all block bodies above the given block number from the database.
    fn remove_bodies_above(
        &self,
        block: BlockNumber,
        remove_from: StorageLocation,
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
        execution_outcome: ExecutionOutcome<Self::Receipt>,
        hashed_state: HashedPostStateSorted,
        trie_updates: TrieUpdates,
    ) -> ProviderResult<()>;
}
