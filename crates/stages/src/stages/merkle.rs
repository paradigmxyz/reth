use crate::{
    db::Transaction, trie::DBTrieLoader, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::consensus;
use std::fmt::Debug;
use tracing::*;

const MERKLE_EXECUTION: StageId = StageId("MerkleExecuteStage");
const MERKLE_UNWIND: StageId = StageId("MerkleUnwindStage");

/// Merkle stage uses input from [AccountHashingStage] and [StorageHashingStage] stages
/// and calculated intermediate hashed and state root.
/// This stage depends on the Account and Storage stages. It will be executed after them during
/// execution, and before them during unwinding.
#[derive(Debug)]
pub struct MerkleStage {
    /// Flag if true would do `execute` but skip unwind but if it false it would skip execution but
    /// do unwind.
    pub is_execute: bool,
    /// The threshold for switching from incremental trie building
    /// of changes to whole rebuild. Num of transitions.
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for MerkleStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        if self.is_execute {
            MERKLE_EXECUTION
        } else {
            MERKLE_UNWIND
        }
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if !self.is_execute {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
        }

        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        // TODO: transform into Transaction helper method
        let previous_numhash = tx.get_block_numhash(previous_stage_progress)?;
        let block_root = tx
            .get::<tables::Headers>(previous_numhash)?
            .ok_or_else(|| {
                StageError::DatabaseIntegrity(DatabaseIntegrityError::Header {
                    number: previous_stage_progress,
                    hash: previous_numhash.hash(),
                })
            })?
            .state_root;

        let current_numhash = tx.get_block_numhash(stage_progress)?;
        let mut current_root = tx
            .get::<tables::Headers>(current_numhash)?
            .ok_or_else(|| {
                StageError::DatabaseIntegrity(DatabaseIntegrityError::Header {
                    number: previous_stage_progress,
                    hash: current_numhash.hash(),
                })
            })?
            .state_root;

        let trie_root = if to_transition - from_transition > self.clean_threshold {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Rebuilding trie");
            // if there are more blocks than threshold it is faster to rebuild the trie
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;

            let loader = DBTrieLoader::default();
            loader.calculate_root(tx).map_err(|e| StageError::Fatal(Box::new(e)))?
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Updating trie");
            // Iterate over changeset (similar to Hashing stages) and take new values
            let loader = DBTrieLoader::default();
            loader
                .update_root(tx, &mut current_root, from_transition, to_transition)
                .map_err(|e| StageError::Fatal(Box::new(e)))?
        };

        if block_root != trie_root {
            warn!(target: "sync::stages::merkle::exec", ?previous_stage_progress, got = ?block_root, expected = ?trie_root, "Block's root state failed verification");
            return Err(StageError::Validation {
                block: previous_stage_progress,
                error: consensus::Error::BodyStateRootDiff { got: block_root, expected: trie_root },
            })
        }

        info!(target: "sync::stages::merkle::exec", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        if self.is_execute {
            info!(target: "sync::stages::merkle::exec", "Stage is always skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        info!(target: "sync::stages::merkle::unwind", "Stage finished");
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
