use alloy_consensus::Header;
use alloy_primitives::BlockNumber;
use reth_db_api::models::StoredBlockBodyIndices;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::SealedBlockWithSenders;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};
use std::ops::RangeInclusive;

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

/// A trait for managing the execution results of blocks in a blockchain.
/// Extends the [`BlockWriter`] trait to provide additional functionality for
/// handling ranges of blocks and their execution results.
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

/// A trait that defines methods for writing block data to the database.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BlockWriter: Send + Sync {
    /// The body this writer can write.
    type Body: Send + Sync;

    /// Insert full block and make it canonical. Parent tx num and transition id is taken from
    /// parent block in database.
    ///
    /// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
    /// transition in the block.
    fn insert_block(
        &self,
        block: SealedBlockWithSenders<Header, Self::Body>,
        write_transactions_to: StorageLocation,
    ) -> ProviderResult<StoredBlockBodyIndices>;

    /// Appends a batch of block bodies extending the canonical chain. This is invoked during
    /// `Bodies` stage and does not write to `TransactionHashNumbers` and `TransactionSenders`
    /// tables which are populated on later stages.
    ///
    /// Bodies are passed as [`Option`]s, if body is `None` the corresponding block is empty.
    fn append_block_bodies(
        &self,
        bodies: Vec<(BlockNumber, Option<Self::Body>)>,
        write_transactions_to: StorageLocation,
    ) -> ProviderResult<()>;

    /// Appends a batch of sealed blocks to the blockchain, including sender information, and
    /// updates the post-state.
    ///
    /// This method inserts the blocks into the database and updates the state using
    /// the provided execution outcome, hashed post-state, and trie updates.
    ///
    /// # Parameters
    ///
    /// - `blocks`: A vector of `SealedBlockWithSenders` instances representing the blocks to
    ///   append, including their headers, bodies, and sender information.
    /// - `execution_outcome`: The result of executing the appended blocks, used to update the
    ///   execution state.
    /// - `hashed_state`: The hashed representation of the post-state after applying the blocks.
    /// - `trie_updates`: State trie updates to be applied to reflect the new post-state.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if any operation fails.
    fn append_blocks_with_state(
        &self,
        blocks: Vec<SealedBlockWithSenders<Header, Self::Body>>,
        execution_outcome: ExecutionOutcome,
        hashed_state: HashedPostStateSorted,
        trie_updates: TrieUpdates,
    ) -> ProviderResult<()>;
}
