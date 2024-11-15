use alloy_primitives::BlockNumber;
use reth_db_api::models::StoredBlockBodyIndices;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::SealedBlockWithSenders;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};
use std::ops::RangeInclusive;

/// BlockExecution Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockExecutionWriter: BlockWriter + Send + Sync {
    /// Take range of blocks and its execution result
    fn take_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain>;

    /// Remove range of blocks and its execution result
    fn remove_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
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
    type Body: Send + Sync;

    /// Insert full block and make it canonical. Parent tx num and transition id is taken from
    /// parent block in database.
    ///
    /// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
    /// transition in the block.
    fn insert_block(&self, block: SealedBlockWithSenders)
        -> ProviderResult<StoredBlockBodyIndices>;

    /// Appends a batch of block bodies extending the canonical chain. This is invoked during
    /// `Bodies` stage and does not write to `TransactionHashNumbers` and `TransactionSenders`
    /// tables which are populated on later stages.
    ///
    /// Bodies are passed as [`Option`]s, if body is `None` the corresponding block is empty.
    fn append_block_bodies(
        &self,
        bodies: impl Iterator<Item = (BlockNumber, Option<Self::Body>)>,
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
        blocks: Vec<SealedBlockWithSenders>,
        execution_outcome: ExecutionOutcome,
        hashed_state: HashedPostStateSorted,
        trie_updates: TrieUpdates,
    ) -> ProviderResult<()>;
}
