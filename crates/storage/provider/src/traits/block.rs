use crate::{Chain, ExecutionOutcome};
use reth_db_api::models::StoredBlockBodyIndices;
use reth_primitives::{BlockNumber, SealedBlockWithSenders};
use reth_prune_types::PruneModes;
use reth_storage_api::BlockReader;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostState};
use std::ops::RangeInclusive;

/// BlockExecution Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockExecutionWriter: BlockWriter + BlockReader + Send + Sync {
    /// Get range of blocks and its execution result
    fn get_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain> {
        self.get_or_take_block_and_execution_range::<false>(range)
    }

    /// Take range of blocks and its execution result
    fn take_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain> {
        self.get_or_take_block_and_execution_range::<true>(range)
    }

    /// Return range of blocks and its execution result
    fn get_or_take_block_and_execution_range<const TAKE: bool>(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain>;
}

/// Block Writer
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockWriter: Send + Sync {
    /// Insert full block and make it canonical. Parent tx num and transition id is taken from
    /// parent block in database.
    ///
    /// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
    /// transition in the block.
    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
        prune_modes: Option<&PruneModes>,
    ) -> ProviderResult<StoredBlockBodyIndices>;

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
    /// - `prune_modes`: Optional pruning configuration.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if any operation fails.
    fn append_blocks_with_state(
        &self,
        blocks: Vec<SealedBlockWithSenders>,
        execution_outcome: ExecutionOutcome,
        hashed_state: HashedPostState,
        trie_updates: TrieUpdates,
        prune_modes: Option<&PruneModes>,
    ) -> ProviderResult<()>;
}
