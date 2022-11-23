use crate::{error::StageError, id::StageId};
use async_trait::async_trait;
use reth_interfaces::db::{DBContainer, Database};
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
    /// Whether or not the stage reached the tip of the chain.
    pub reached_tip: bool,
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
/// Stages receive a [`DBContainer`] which manages the lifecycle of a transaction, such
/// as when to commit / reopen a new one etc.
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>;
}
