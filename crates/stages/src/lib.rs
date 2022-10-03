#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Staged syncing primitives for reth.
//!
//! See [Stage] and [Pipeline].

use async_trait::async_trait;
use reth_primitives::U64;
use thiserror::Error;

mod pipeline;
pub use pipeline::*;

/// Stage execution input, see [Stage::execute].
#[derive(Clone, Copy, Debug)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the block number it reached.
    pub previous_stage: Option<(StageId, U64)>,
    /// The progress of this stage the last time it was executed.
    pub stage_progress: Option<U64>,
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Clone, Copy, Debug)]
pub struct UnwindInput {
    /// The current highest block of the stage.
    pub stage_progress: U64,
    /// The block to unwind to.
    pub unwind_to: U64,
    /// The bad block that caused the unwind, if any.
    pub bad_block: Option<U64>,
}

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq)]
pub struct ExecOutput {
    /// How far the stage got.
    pub stage_progress: U64,
    /// Whether or not the stage is done.
    pub done: bool,
    /// Whether or not the stage reached the tip of the chain.
    pub reached_tip: bool,
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq)]
pub struct UnwindOutput {
    /// The block at which the stage has unwound to.
    pub stage_progress: U64,
}

/// A stage execution error.
#[derive(Error, Debug)]
pub enum StageError {
    /// The stage encountered a state validation error.
    ///
    /// TODO: This depends on the consensus engine and should include the validation failure reason
    #[error("Stage encountered a validation error.")]
    Validation,
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// The ID of a stage.
///
/// Each stage ID must be unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageId(pub &'static str);

/// A stage is a segmented part of the syncing process of the node.
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
#[async_trait]
pub trait Stage {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut dyn DbTransaction,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut dyn DbTransaction,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>;
}

/// TODO: Stand-in for database-related abstractions.
pub trait DbTransaction {}
