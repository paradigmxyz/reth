use crate::{error::StageError, id::StageId};
use async_trait::async_trait;
use reth_db::database::Database;
use reth_primitives::BlockNumber;
use reth_provider::Transaction;
use std::{
    cmp::{max, min},
    ops::RangeInclusive,
};

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the block number it reached.
    pub previous_stage: Option<(StageId, BlockNumber)>,
    /// The progress of this stage the last time it was executed.
    pub stage_progress: Option<BlockNumber>,
}

impl ExecInput {
    /// Return the progress of the stage or default.
    pub fn stage_progress(&self) -> BlockNumber {
        self.stage_progress.unwrap_or_default()
    }

    /// Return the progress of the previous stage or default.
    pub fn previous_stage_progress(&self) -> BlockNumber {
        self.previous_stage.as_ref().map(|(_, num)| *num).unwrap_or_default()
    }

    /// Return next block range that needs to be executed.
    pub fn next_block_range(&self) -> RangeInclusive<BlockNumber> {
        let (range, _) = self.next_block_range_with_threshold(u64::MAX);
        range
    }

    /// Return true if this is the first block range to execute.
    pub fn is_first_range(&self) -> bool {
        self.stage_progress.is_none()
    }

    /// Return the next block range to execute.
    /// Return pair of the block range and if this is final block range.
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.stage_progress.unwrap_or_default();
        // +1 is to skip present block and always start from block number 1, not 0.
        let start = current_block + 1;
        let target = self.previous_stage_progress();

        let end = min(target, current_block.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest block of the stage.
    pub stage_progress: BlockNumber,
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
        let end = self.stage_progress;

        start = max(start, end.saturating_sub(threshold));

        let unwind_to = start - 1;

        let is_final_range = unwind_to == self.unwind_to;
        (start..=end, unwind_to, is_final_range)
    }
}

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub stage_progress: BlockNumber,
    /// Whether or not the stage is done.
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given block number.
    pub fn done(stage_progress: BlockNumber) -> Self {
        Self { stage_progress, done: true }
    }
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The block at which the stage has unwound to.
    pub stage_progress: BlockNumber,
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
/// Stages receive [`Transaction`] which manages the lifecycle of a transaction,
/// such as when to commit / reopen a new one etc.
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
