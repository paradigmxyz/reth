use alloy_primitives::BlockNumber;
use reth_db_api::models::StoredBlockBodyIndices;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::SealedBlockWithSenders;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};
use std::ops::RangeInclusive;

use crate::providers::metrics;

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
    /// Insert full block and make it canonical. Parent tx num and transition id is taken from
    /// parent block in database.
    ///
    /// Return [StoredBlockBodyIndices] that contains indices of the first and last transactions and
    /// transition in the block.
    fn insert_block(&self, block: SealedBlockWithSenders)
        -> ProviderResult<StoredBlockBodyIndices>;

    /// Inserts additional block data that is not part of the header or transactions.
    ///
    /// This function handles the insertion of ommers, withdrawals, and requests data.
    ///
    /// # Arguments
    /// - `block` - The `SealedBlockWithSenders` containing the additional data to be inserted
    /// - `durations_recorder` - A mutable reference to a `DurationsRecorder` for performance
    ///   tracking
    ///
    /// # Returns
    /// - `ProviderResult<()>` - Ok if the insertion was successful, or an error if it failed
    ///
    /// # Tables modified
    /// - `BlockOmmers` - If the block contains ommers
    /// - `BlockWithdrawals` - If the block contains withdrawals
    /// - `BlockRequests` - If the block contains requests
    fn insert_block_additional_data(
        &self,
        block: SealedBlockWithSenders,
        durations_recorder: &mut metrics::DurationsRecorder,
    ) -> ProviderResult<()>;

    /// Inserts block header data and transaction data, making the block canonical.
    ///
    /// This function handles the insertion of header-related data and transaction data.
    /// It calculates the total difficulty and inserts all transaction-related information.
    ///
    /// # Arguments
    /// - `block` - The `SealedBlockWithSenders` to be inserted
    /// - `durations_recorder` - A mutable reference to a `DurationsRecorder` for performance
    ///   tracking
    ///
    /// # Returns
    /// - `ProviderResult<StoredBlockBodyIndices>` - The indices of the transactions in the block
    ///
    /// # Tables modified
    /// - `CanonicalHeaders` - Stores the canonical header hash for each block number
    /// - `Headers` - Stores the full header data
    /// - `HeaderNumbers` - Maps block hashes to block numbers
    /// - `HeaderTerminalDifficulties` - Stores the total difficulty for each block
    /// - `Transactions` - Stores transaction data
    /// - `TransactionSenders` - Stores transaction senders (if not fully pruned)
    /// - `TransactionHashNumbers` - Maps transaction hashes to transaction numbers (if not fully
    ///   pruned)
    /// - `BlockBodyIndices` - Stores the indices of transactions for each block
    /// - `TransactionBlocks` - Maps transaction numbers to block numbers
    fn insert_block_header_and_transaction_data(
        &self,
        block: SealedBlockWithSenders,
        durations_recorder: &mut metrics::DurationsRecorder,
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
