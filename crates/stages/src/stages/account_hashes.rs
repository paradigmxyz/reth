use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::AccountBeforeTx,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address};
use tracing::*;

const ACCOUNT_HASHING: StageId = StageId("AccountHashing");

/// The account hashing stage iterates over account states,
/// hashes their addresses and inserts entries to the
/// [`HashedAccount`][reth_interfaces::db::tables::HashedAccount] table.
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

        // TODO: use macro here
        if max_block_num <= stage_progress {
            info!(target: "sync::stages::account_hashing", target = max_block_num, stage_progress, "Target block already reached");
            return Ok(ExecOutput { stage_progress, done: true })
        }

        // We get the last transition from block `stage_progress` and add one to get the transition
        // from the next block.
        let start_transition = tx.get_block_transition_by_num(stage_progress)? + 1;
        let end_transition = tx.get_block_transition_by_num(max_block_num)?;

        if start_transition > end_transition {
            // No transitions to walk over
            info!(target: "sync::stages::account_hashing", start_transition, end_transition, "Target transition already reached");
            return Ok(ExecOutput { stage_progress: max_block_num, done: true })
        } else if end_transition - start_transition > self.clean_threshold || stage_progress == 0 {
            // There are too many transitions, so we rehash all accounts
            tx.clear::<tables::HashedAccount>()?;

            let mut account_cursor = tx.cursor::<tables::PlainAccountState>()?;
            let mut hashed_account_cursor = tx.cursor_mut::<tables::HashedAccount>()?;

            let mut walker = account_cursor.walk(Address::zero())?;

            while let Some((address, account)) = walker.next().transpose()? {
                let hashed_address = keccak256(address);
                hashed_account_cursor.append(hashed_address, account)?;
            }

            return Ok(ExecOutput { stage_progress: previous_stage_progress, done: true })
        }

        // Acquire the PlainAccount cursor
        let mut acc_cursor = tx.cursor::<tables::PlainAccountState>()?;
        // Acquire the AccountChangeSet cursor
        let mut acc_changeset_cursor = tx.cursor::<tables::AccountChangeSet>()?;
        // Acquire the cursor for inserting elements
        let mut hashed_acc_cursor = tx.cursor_mut::<tables::HashedAccount>()?;

        let mut walker = acc_changeset_cursor.walk(start_transition)?.take_while(|ch_entry| {
            ch_entry.as_ref().map(|(tid, _)| tid <= &end_transition).unwrap_or_default()
        });

        // Iterate over transactions in chunks
        info!(target: "sync::stages::account_hashing", start_transition, end_transition, "Hashing accounts");
        while let Some((_, AccountBeforeTx { address, .. })) = walker.next().transpose()? {
            // Query updated account
            // If it was not deleted, upsert hashed account table
            if let Some((_, acc)) = acc_cursor.seek_exact(address)? {
                hashed_acc_cursor.upsert(keccak256(address), acc)?;
            // If account was deleted, delete entry from the hashed accounts table
            } else if hashed_acc_cursor.seek_exact(keccak256(address))?.is_some() {
                hashed_acc_cursor.delete_current()?;
            }
        }

        let done = max_block_num >= previous_stage_progress;
        info!(target: "sync::stages::account_hashing", stage_progress = max_block_num, done, "Sync iteration finished");

        Ok(ExecOutput { stage_progress: max_block_num, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Get the last transition of the `unwind_to` block to know when unwinding should stop
        let end_transition = tx.get_block_transition_by_num(input.unwind_to)?;

        let mut hashed_acc_cursor = tx.cursor_mut::<tables::HashedAccount>()?;
        let mut acc_changeset_cursor = tx.cursor::<tables::AccountChangeSet>()?;

        let mut entry = acc_changeset_cursor.last()?;
        while let Some((tid, ref acc_before_tx)) = entry {
            if tid <= end_transition {
                break
            }
            let hashed_addr = keccak256(acc_before_tx.address);
            if let Some(acc) = acc_before_tx.info {
                hashed_acc_cursor.upsert(hashed_addr, acc)?;
            } else if hashed_acc_cursor.seek_exact(hashed_addr)?.is_some() {
                hashed_acc_cursor.delete_current()?;
            }

            entry = acc_changeset_cursor.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
