use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::database::Database;
use std::fmt::Debug;
use tracing::*;

/// The [`StageId`] of the merkle hashing execution stage.
pub const MERKLE_EXECUTION: StageId = StageId("MerkleExecute");

/// The [`StageId`] of the merkle hashing unwind stage.
pub const MERKLE_UNWIND: StageId = StageId("MerkleUnwind");

/// The merkle hashing stage uses input from
/// [`AccountHashingStage`][crate::stages::AccountHashingStage] and
/// [`StorageHashingStage`][crate::stages::AccountHashingStage] to calculate intermediate hashes
/// and state roots.
///
/// This stage should be run with the above two stages, otherwise it is a no-op.
///
/// This stage is split in two: one for calculating hashes and one for unwinding.
///
/// When run in execution, it's going to be executed AFTER the hashing stages, to generate
/// the state root. When run in unwind mode, it's going to be executed BEFORE the hashing stages,
/// so that it unwinds the intermediate hashes based on the unwound hashed state from the hashing
/// stages. The order of these two variants is important. The unwind variant should be added to the
/// pipeline before the execution variant.
///
/// An example pipeline to only hash state would be:
///
/// - [`MerkleStage::Unwind`]
/// - [`AccountHashingStage`][crate::stages::AccountHashingStage]
/// - [`StorageHashingStage`][crate::stages::StorageHashingStage]
/// - [`MerkleStage::Execution`]
#[derive(Debug)]
pub enum MerkleStage {
    /// The execution portion of the hashing stage.
    Execution,
    /// The unwind portion of the hasing stage.
    Unwind,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for MerkleStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        match self {
            MerkleStage::Execution => MERKLE_EXECUTION,
            MerkleStage::Unwind => MERKLE_UNWIND,
        }
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if matches!(self, MerkleStage::Unwind) {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
        }

        // Iterate over changeset (similar to Hashing stages) and take new values

        info!(target: "sync::stages::merkle::exec", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        if matches!(self, MerkleStage::Execution) {
            info!(target: "sync::stages::merkle::exec", "Stage is always skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        info!(target: "sync::stages::merkle::unwind", "Stage finished");
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
