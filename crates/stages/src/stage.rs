use crate::error::StageError;
use async_trait::async_trait;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, PruneMode, TxNumber,
};
use reth_provider::{BlockReader, DatabaseProviderRW, ProviderError};
use std::{
    cmp::{max, min},
    ops::RangeInclusive,
};

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    pub checkpoint: Option<StageCheckpoint>,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
    pub fn checkpoint(&self) -> StageCheckpoint {
        self.checkpoint.unwrap_or_default()
    }

    /// Return the next block number after the current
    /// +1 is needed to skip the present block and always start from block number 1, not 0.
    pub fn next_block(&self) -> BlockNumber {
        let current_block = self.checkpoint();
        current_block.block_number + 1
    }

    /// Returns `true` if the target block number has already been reached.
    pub fn target_reached(&self) -> bool {
        self.checkpoint().block_number >= self.target()
    }

    /// Return the target block number or default.
    pub fn target(&self) -> BlockNumber {
        self.target.unwrap_or_default()
    }

    /// Return next block range that needs to be executed.
    pub fn next_block_range(&self) -> RangeInclusive<BlockNumber> {
        let (range, _) = self.next_block_range_with_threshold(u64::MAX);
        range
    }

    /// Return true if this is the first block range to execute.
    pub fn is_first_range(&self) -> bool {
        self.checkpoint.is_none()
    }

    /// Return the next block range to execute.
    /// Return pair of the block range and if this is final block range.
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }

    /// Return the next block range determined the number of transactions within it.
    /// This function walks the the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    pub fn next_block_range_with_transaction_threshold<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        tx_threshold: u64,
    ) -> Result<(RangeInclusive<TxNumber>, RangeInclusive<BlockNumber>, bool), StageError> {
        let start_block = self.next_block();
        let start_block_body = provider
            .block_body_indices(start_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(start_block))?;

        let target_block = self.target();

        let first_tx_number = start_block_body.first_tx_num();
        let mut last_tx_number = start_block_body.last_tx_num();
        let mut end_block_number = start_block;
        let mut body_indices_cursor =
            provider.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;
        for entry in body_indices_cursor.walk_range(start_block..=target_block)? {
            let (block, body) = entry?;
            last_tx_number = body.last_tx_num();
            end_block_number = block;
            let tx_count = (first_tx_number..=last_tx_number).count() as u64;
            if tx_count > tx_threshold {
                break
            }
        }
        let is_final_range = end_block_number >= target_block;
        Ok((first_tx_number..=last_tx_number, start_block..=end_block_number, is_final_range))
    }
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    pub unwind_to: BlockNumber,
    /// The bad block that caused the unwind, if any.
    pub bad_block: Option<BlockNumber>,
}

impl UnwindInput {
    /// Return next block range that needs to be unwound.
    pub fn unwind_block_range(&self) -> RangeInclusive<BlockNumber> {
        self.unwind_block_range_with_threshold(u64::MAX).0
    }

    /// Return the next block range to unwind and the block we're unwinding to.
    pub fn unwind_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, BlockNumber, bool) {
        // +1 is to skip the block we're unwinding to
        let mut start = self.unwind_to + 1;
        let end = self.checkpoint;

        start = max(start, end.block_number.saturating_sub(threshold));

        let unwind_to = start - 1;

        let is_final_range = unwind_to == self.unwind_to;
        (start..=end.block_number, unwind_to, is_final_range)
    }
}

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub checkpoint: StageCheckpoint,
    /// Whether or not the stage is done.
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given place.
    pub fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: true }
    }
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
///
/// Stages receive [`DatabaseProviderRW`].
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}

/// Prune target.
#[derive(Debug, Clone, Copy)]
pub enum PruneTarget {
    /// Prune all blocks, i.e. not save any data.
    All,
    /// Prune blocks up to the specified block number, inclusive.
    Block(BlockNumber),
}

impl PruneTarget {
    /// Returns new target to prune towards, according to stage prune mode [PruneMode]
    /// and current head [BlockNumber].
    pub fn new(prune_mode: PruneMode, head: BlockNumber) -> Self {
        match prune_mode {
            PruneMode::Full => PruneTarget::All,
            PruneMode::Distance(distance) => {
                Self::Block(head.saturating_sub(distance).saturating_sub(1))
            }
            PruneMode::Before(before_block) => Self::Block(before_block.saturating_sub(1)),
        }
    }

    /// Returns true if the target is [PruneTarget::All], i.e. prune all blocks.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }
}
