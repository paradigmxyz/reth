use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO},
    database::Database,
    tables,
    transaction::{DbTx},
};
use reth_primitives::TransitionId;
use tracing::*;

const ACCOUNT_HASHING: StageId = StageId("AccountHashing");

#[derive(Debug)]
pub struct AccountHashingStage {
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
    /// The threshold for switching from incremental hashing
    /// of changes to whole account hashing
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for AccountHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        ACCOUNT_HASHING
    }

    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let previous_stage_progress = input.previous_stage_progress();
        let stage_progress = input.stage_progress.unwrap_or_default();
        let max_block_num = previous_stage_progress.min(stage_progress + self.commit_threshold);

        if max_block_num <= stage_progress {
            info!(target: "sync::stages::account_hashing", target = max_block_num, stage_progress, "Target block already reached");
            return Ok(ExecOutput { stage_progress, done: true })
        }

                // Look up the start index for the transaction range
                let start_tx_index = tx.get_block_body_by_num(stage_progress + 1)?.start_tx_id;

                // Look up the end index for transaction range (inclusive)
                let end_tx_index = tx.get_block_body_by_num(max_block_num)?.last_tx_index();
        
                // Acquire the cursor for transaction-transition mapping
                let mut tx_transition_cursor = tx.cursor::<tables::TxTransitionIndex>()?;
        
                let start_transition: TransitionId =
                    tx_transition_cursor.seek_exact(start_tx_index)?.unwrap_or_default().1;
                let end_transition: TransitionId =
                    tx_transition_cursor.seek_exact(end_tx_index)?.unwrap_or_default().1;


        todo!();
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        unimplemented!();
    }
}
