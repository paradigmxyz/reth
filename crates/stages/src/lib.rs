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
use reth_db::mdbx;
use reth_primitives::U64;
use std::fmt::Display;
use thiserror::Error;

mod pipeline;
pub use pipeline::*;

/// Stage execution input, see [Stage::execute].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the block number it reached.
    pub previous_stage: Option<(StageId, U64)>,
    /// The progress of this stage the last time it was executed.
    pub stage_progress: Option<U64>,
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest block of the stage.
    pub stage_progress: U64,
    /// The block to unwind to.
    pub unwind_to: U64,
    /// The bad block that caused the unwind, if any.
    pub bad_block: Option<U64>,
}

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub stage_progress: U64,
    /// Whether or not the stage is done.
    pub done: bool,
    /// Whether or not the stage reached the tip of the chain.
    pub reached_tip: bool,
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
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
    #[error("Stage encountered a validation error in block {block}.")]
    Validation {
        /// The block that failed validation.
        block: U64,
    },
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
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
#[async_trait]
pub trait Stage<'db, E>: Send + Sync
where
    E: mdbx::EnvironmentKind,
{
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute<'tx>(
        &mut self,
        tx: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>
    where
        'db: 'tx;

    /// Unwind the stage.
    async fn unwind<'tx>(
        &mut self,
        tx: &mut mdbx::Transaction<'tx, mdbx::RW, E>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>
    where
        'db: 'tx;
}

/// The ID of a stage.
///
/// Each stage ID must be unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageId(pub &'static str);

impl Display for StageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StageId {
    /// Get the last committed progress of this stage.
    pub fn get_progress<'db, K, E>(
        &self,
        tx: &mdbx::Transaction<'db, K, E>,
    ) -> Result<Option<U64>, mdbx::Error>
    where
        K: mdbx::TransactionKind,
        E: mdbx::EnvironmentKind,
    {
        // TODO: Clean up when we get better database abstractions
        let bytes: Option<Vec<u8>> = tx.get(&tx.open_db(Some("SyncStage"))?, self.0.as_ref())?;

        Ok(bytes.map(|b| U64::from_big_endian(b.as_ref())))
    }

    /// Save the progress of this stage.
    pub fn save_progress<'db, E>(
        &self,
        tx: &mdbx::Transaction<'db, mdbx::RW, E>,
        block: U64,
    ) -> Result<(), mdbx::Error>
    where
        E: mdbx::EnvironmentKind,
    {
        // TODO: Clean up when we get better database abstractions
        tx.put(
            &tx.open_db(Some("SyncStage"))?,
            self.0,
            block.0[0].to_be_bytes(),
            mdbx::WriteFlags::UPSERT,
        )
    }
}
