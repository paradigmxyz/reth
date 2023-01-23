use crate::{
    db::Transaction, exec_or_return, ExecAction, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use tracing::*;

const TRANSACTION_LOOKUP: StageId = StageId("TransactionLookup");

/// The transaction lookup stage.
///
/// This stage walks over the bodies table, and sets the transaction hash of each transaction in a
/// block to the correesponding `TransitionId` at each block. This is written to the
/// [`tables::TxHashNumber`] This is used for looking up changesets via the transaction hash.
#[derive(Debug)]
pub struct TransactionLookupStage {
    /// The number of table entries to commit at once
    pub commit_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TransactionLookupStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        TRANSACTION_LOOKUP
    }

    /// Write total difficulty entries
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let ((start_block, end_block), capped) =
            exec_or_return!(input, self.commit_threshold, "sync::stages::transaction_lookup");

        debug!(target: "sync::stages::transaction_lookup", start_block, end_block, "Commencing sync");

        let mut cursor_bodies = tx.cursor_read::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;
        let start_key = tx.get_block_numhash(start_block)?;

        let bodies = cursor_bodies
            .walk(start_key)?
            .take_while(|entry| {
                entry
                    .as_ref()
                    .map(|(block_num_hash, _)| block_num_hash.number() <= end_block)
                    .unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (_, body) in bodies {
            let transactions = tx_cursor
                .walk(body.start_tx_id)?
                .take(body.tx_count as usize)
                .collect::<Result<Vec<_>, _>>()?;

            for (id, transaction) in transactions {
                tx.put::<tables::TxHashNumber>(transaction.hash(), id)?;
            }
        }

        let done = !capped;
        info!(target: "sync::stages::transaction_lookup", stage_progress = end_block, done, "Sync iteration finished");
        Ok(ExecOutput { done, stage_progress: end_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::transaction_lookup", to_block = input.unwind_to, "Unwinding");
        // Cursors to unwind tx hash to number
        let mut body_cursor = tx.cursor_write::<tables::BlockBodies>()?;
        let mut tx_hash_number_cursor = tx.cursor_write::<tables::TxHashNumber>()?;
        let mut transaction_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut rev_walker = body_cursor.walk_back(None)?;
        while let Some((key, body)) = rev_walker.next().transpose()? {
            if key.number() <= input.unwind_to {
                break
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_id_range() {
                // First delete the transaction and hash to id mapping
                if let Some((_, transaction)) = transaction_cursor.seek_exact(tx_id)? {
                    if tx_hash_number_cursor.seek_exact(transaction.hash)?.is_some() {
                        tx_hash_number_cursor.delete_current()?;
                    }
                }
            }
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
