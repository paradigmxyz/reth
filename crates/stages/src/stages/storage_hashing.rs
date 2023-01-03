use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Address, HashedStorageEntry, TransitionId};
use std::fmt::Debug;
use tracing::*;

const STORAGE_HASHING: StageId = StageId("StorageHashing");

/// The storage hashing stage iterates over existing account
/// storages, and with them populates the
/// [`HashedStorage`][reth_interfaces::db::tables::HashedStorage] table.
#[derive(Debug)]
pub struct StorageHashingStage {
    /// The size of the chunk for parallel storage hashing
    pub batch_size: usize,
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
    /// The threshold for switching from incremental hashing
    /// of changes to whole storage hashing
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for StorageHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        STORAGE_HASHING
    }

    /// Retrieve the range of transactions to iterate over by querying
    /// [`CumulativeTxCount`][reth_interfaces::db::tables::CumulativeTxCount],
    /// collect transactions within that range,
    /// recover signer for each transaction and store entries in
    /// the [`TxSenders`][reth_interfaces::db::tables::TxSenders] table.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();
        let max_block_num = previous_stage_progress.min(stage_progress + self.commit_threshold);

        if max_block_num <= stage_progress {
            info!(target: "sync::stages::storage_hashing", target = max_block_num, stage_progress, "Target block already reached");
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

        // No transitions to walk over
        if start_transition > end_transition {
            info!(target: "sync::stages::storage_hashing", start_transition, end_transition, "Target transition already reached");
            return Ok(ExecOutput { stage_progress: max_block_num, done: true })
        } else if end_transition - start_transition > self.clean_threshold {
            // There are too many transitions, so we hash all storage entries
            let mut storage_cursor = tx.cursor_dup::<tables::PlainStorageState>()?;
            let mut hashed_storage_cursor = tx.cursor_dup_mut::<tables::HashedStorage>()?;

            let mut walker = storage_cursor.walk(Address::zero())?;

            while let Some((address, entry)) = walker.next().transpose()? {
                let hashed_entry = HashedStorageEntry { key: keccak256(address), ..entry };
                hashed_storage_cursor.append_dup(keccak256(address), hashed_entry)?;
            }
        }

        // Acquire the Storage cursor
        let mut storage_cursor = tx.cursor_dup::<tables::PlainStorageState>()?;
        // Acquire the changeset cursor
        let mut storage_changeset_cursor = tx.cursor::<tables::StorageChangeSet>()?;
        // Acquire the cursor for inserting elements
        let mut hashed_storage_cursor = tx.cursor_dup_mut::<tables::HashedStorage>()?;

        // Walk the transactions from start to end index (inclusive)
        let mut walker =
            storage_changeset_cursor.walk(start_transition.into())?.take_while(|res| {
                res.as_ref()
                    .map(|(k, _)| (*k).transition_id() <= end_transition)
                    .unwrap_or_default()
            });

        // Iterate over transactions in chunks
        info!(target: "sync::stages::storage_hashing", start_transition, end_transition, "Hashing storage");
        while let Some((tid_address, entry)) = walker.next().transpose()? {
            if let Some(current_se) =
                storage_cursor.seek_by_key_subkey(tid_address.address(), entry.key)?
            {
                // Create a new entry with a hashed key
                let hashed_se = HashedStorageEntry { key: keccak256(current_se.key), ..current_se };

                // upsert entry to the table
                hashed_storage_cursor.append_dup(keccak256(tid_address.address()), hashed_se)?;
            } else {
                hashed_storage_cursor.seek_exact(keccak256(tid_address.address()))?;
                hashed_storage_cursor.delete_current()?;
            }
        }

        let done = max_block_num >= previous_stage_progress;
        info!(target: "sync::stages::storage_hashing", stage_progress = max_block_num, done, "Sync iteration finished");
        Ok(ExecOutput { stage_progress: max_block_num, done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        unimplemented!();
        // Lookup latest tx id that we should unwind to
        // let latest_tx_id = tx.get_block_body_by_num(input.unwind_to)?.last_tx_index();
        // tx.unwind_table_by_num::<tables::TxSenders>(latest_tx_id)?;
        // Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
