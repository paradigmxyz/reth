use crate::{
    util::unwind::unwind_table_by_num, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use reth_interfaces::db::{tables, DBContainer, Database, DbCursorRO, DbTx, DbTxMut};
use std::fmt::Debug;

const SENDERS: StageId = StageId("Senders");

#[derive(Debug)]
struct SenderStage {}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for SenderStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        SENDERS
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = db.get_mut();
        let last_block_num =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        let start_block = last_block_num + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or(DatabaseIntegrityError::CannonicalHash { number: start_block })?;

        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut body_walker = body_cursor.walk((start_block, start_hash).into())?;

        let mut senders_cursor = tx.cursor_mut::<tables::TxSenders>()?;
        while let Some((key, tx_count)) = body_walker.next().transpose()? {
            if tx_count == 0 {
                continue
            }

            let cum_tx_count = tx.get::<tables::CumulativeTxCount>(key)?.ok_or(
                DatabaseIntegrityError::CumulativeTxCount {
                    hash: key.hash(),
                    number: key.number(),
                },
            )?;
            let mut tx_cursor = tx.cursor_mut::<tables::Transactions>()?;
            let transactions = tx_cursor
                .walk(cum_tx_count - (tx_count as u64) - 1)?
                .take(tx_count as usize)
                .collect::<Result<Vec<_>, reth_interfaces::db::Error>>()?;

            for (id, transaction) in transactions.iter() {
                // senders_cursor.append(*id, transaction.recover_sender().unwrap());
            }
        }

        Ok(ExecOutput { stage_progress: 0, done: false, reached_tip: false })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let tx = db.get_mut();

        // Look up the hash of the unwind block
        let unwind_hash = tx
            .get::<tables::CanonicalHeaders>(input.unwind_to)?
            .ok_or(DatabaseIntegrityError::CannonicalHeader { number: input.unwind_to })?;

        // Look up the cumulative tx count at unwind block
        let latest_tx = tx
            .get::<tables::CumulativeTxCount>((input.unwind_to, unwind_hash).into())?
            .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                number: input.unwind_to,
                hash: unwind_hash,
            })?;

        unwind_table_by_num::<DB, tables::TxSenders>(tx, latest_tx - 1)?;
        Ok(UnwindOutput { stage_progress: 0 })
    }
}
