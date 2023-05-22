use crate::{
    error::StageError,
    id::StageId,
    stages::{ACCOUNT_HASHING, STORAGE_HASHING},
};
use async_trait::async_trait;
use reth_db::database::Database;
use reth_primitives::{BlockNumber, StageCheckpoint};
use reth_provider::Transaction;
use std::{
    cmp::{max, min},
    fmt::{Display, Formatter},
    ops::RangeInclusive,
};

/// Progress of a stage. Used for metrics/logging purposes.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StageProgress {
    /// Hashing stage progress.
    Hashing(HashingStageProgress),
}

impl StageProgress {
    /// Creates a new [`StageProgress`] from [`StageId`], if any stage-specific progress is known.
    pub fn from_stage_id(stage_id: StageId) -> Option<Self> {
        Some(match stage_id {
            ACCOUNT_HASHING | STORAGE_HASHING => Self::Hashing(HashingStageProgress::default()),
            _ => return None,
        })
    }

    /// Returns the hashing stage progress, if any.
    pub fn hashing(&self) -> Option<HashingStageProgress> {
        #[allow(irrefutable_let_patterns)]
        if let Self::Hashing(hashing) = self {
            Some(*hashing)
        } else {
            None
        }
    }
}

impl Display for StageProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match *self {
            StageProgress::Hashing(hashing) => {
                write!(f, "{hashing}")
            }
        }
    }
}

/// Hashing stage progress.
/// For `execute`, it's measured in accounts or storage entries.
/// For `unwind`, it's measured in changesets.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct HashingStageProgress {
    /// Entries already processed.
    pub entries_processed: u64,
    /// Entries left to process.
    pub entries_total: u64,
}

impl Display for HashingStageProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // write!(f, "{:.1}%", 100.0 * self.entries_processed as f64 / self.entries_total as f64)
        write!(f, "{}/{}", self.entries_processed as f64, self.entries_total as f64)
    }
}

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the checkpoint it reached.
    pub previous_stage: Option<(StageId, StageCheckpoint)>,
    /// The checkpoint of this stage the last time it was executed.
    pub checkpoint: Option<StageCheckpoint>,
    /// The progress of this stage the last time it was executed, if any.
    pub progress: Option<StageProgress>,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
    pub fn checkpoint(&self) -> StageCheckpoint {
        self.checkpoint.unwrap_or_default()
    }

    /// Return the progress of the previous stage or default.
    pub fn previous_stage_checkpoint(&self) -> StageCheckpoint {
        self.previous_stage.map(|(_, checkpoint)| checkpoint).unwrap_or_default()
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
        let current_block = self.checkpoint.unwrap_or_default();
        // +1 is to skip present block and always start from block number 1, not 0.
        let start = current_block.block_number + 1;
        let target = self.previous_stage_checkpoint().block_number;

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    pub checkpoint: StageCheckpoint,
    /// The progress of this stage the last time it was unwound, if any.
    pub progress: Option<StageProgress>,
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
    /// How far the stage got.
    pub progress: Option<StageProgress>,
    /// Whether or not the stage is done.
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given place.
    pub fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, progress: None, done: true }
    }
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    pub checkpoint: StageCheckpoint,
    /// The progress at which the stage has unwound to.
    pub progress: Option<StageProgress>,
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
