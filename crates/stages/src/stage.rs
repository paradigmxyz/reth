use std::ops::RangeInclusive;

use crate::{db::Transaction, error::StageError, id::StageId};
use async_trait::async_trait;
use reth_db::database::Database;
use reth_primitives::BlockNumber;

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the block number it reached.
    pub previous_stage: Option<(StageId, BlockNumber)>,
    /// The progress of this stage the last time it was executed.
    pub stage_progress: Option<BlockNumber>,
}

impl ExecInput {
    /// Return the progress of the previous stage or default.
    pub fn previous_stage_progress(&self) -> BlockNumber {
        self.previous_stage.as_ref().map(|(_, num)| *num).unwrap_or_default()
    }

    /// Return next execution action.
    ///
    /// [ExecAction::Done] is returned if there are no blocks to execute in this stage.
    /// [ExecAction::Run] is returned if the stage should proceed with execution.
    pub fn next_action(&self, max_threshold: Option<u64>) -> ExecAction {
        // Extract information about the stage progress
        let stage_progress = self.stage_progress.unwrap_or_default();
        let previous_stage_progress = self.previous_stage_progress();

        let start_block = stage_progress + 1;
        let end_block = match max_threshold {
            Some(threshold) => previous_stage_progress.min(stage_progress + threshold),
            None => previous_stage_progress,
        };
        let capped = end_block < previous_stage_progress;

        if start_block <= end_block {
            ExecAction::Run { range: start_block..=end_block, capped }
        } else {
            ExecAction::Done { stage_progress, target: end_block }
        }
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

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub stage_progress: BlockNumber,
    /// Whether or not the stage is done.
    pub done: bool,
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The block at which the stage has unwound to.
    pub stage_progress: BlockNumber,
}

/// Controls whether a stage should continue execution or not.
#[derive(Debug)]
pub enum ExecAction {
    /// The stage should continue with execution.
    Run {
        /// The execution block range
        range: RangeInclusive<BlockNumber>,
        /// The flag indicating whether the range was capped
        /// by some max blocks parameter
        capped: bool,
    },
    /// The stage should terminate since there are no blocks to execute.
    Done {
        /// The current stage progress
        stage_progress: BlockNumber,
        /// The execution target provided in [ExecInput].
        target: BlockNumber,
    },
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

/// Get the next execute action for the stage. Return if the stage has no
/// blocks to process.
macro_rules! exec_or_return {
    ($input: expr, $log_target: literal) => {
        match $input.next_action(None) {
            // Next action cannot be capped without a threshold.
            ExecAction::Run { range, capped: _capped } => range.into_inner(),
            ExecAction::Done { stage_progress, target } => {
                info!(target: $log_target, stage_progress, target, "Target block already reached");
                return Ok(ExecOutput { stage_progress, done: true })
            }
        }
    };
    ($input: expr, $threshold: expr, $log_target: literal) => {
        match $input.next_action(Some($threshold)) {
            ExecAction::Run { range, capped } => (range.into_inner(), capped),
            ExecAction::Done { stage_progress, target } => {
                info!(target: $log_target, stage_progress, target, "Target block already reached");
                return Ok(ExecOutput { stage_progress, done: true })
            }
        }
    };
}

pub(crate) use exec_or_return;
