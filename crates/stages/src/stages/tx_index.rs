use crate::{
    util::unwind::unwind_table_by_num_hash, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_interfaces::db::{tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut};
use std::fmt::Debug;

const TX_INDEX: StageId = StageId("TX_INDEX");

/// The cumulative transaction index stage
/// implementation for staged sync.
#[derive(Debug)]
pub struct TxIndex;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TxIndex {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        TX_INDEX
    }

    /// Execute the stage
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = db.get_mut();

        let last_block = input.stage_progress.unwrap_or_default();
        let last_hash = tx
            .get::<tables::CanonicalHeaders>(last_block)?
            .ok_or_else(|| DatabaseIntegrityError::NoCannonicalHeader { number: last_block })?;

        let start_block = last_block + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or_else(|| DatabaseIntegrityError::NoCannonicalHeader { number: start_block })?;

        let max_block = input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();

        let mut cursor = tx.cursor_mut::<tables::CumulativeTxCount>()?;
        let (_, mut count) =
            cursor.seek_exact((last_block, last_hash).into())?.ok_or_else(|| {
                DatabaseIntegrityError::NoCumulativeTxCount { number: last_block, hash: last_hash }
            })?;

        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let walker = body_cursor.walk((start_block, start_hash).into())?;
        let entries = walker
            .take_while(|b| b.as_ref().map(|(k, _)| k.number() <= max_block).unwrap_or_default());
        for entry in entries {
            let (key, tx_count) = entry?;
            count += tx_count as u64;
            cursor.append(key, count)?;
        }

        Ok(ExecOutput { done: false, reached_tip: false, stage_progress: 0 })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let tx = db.get_mut();
        unwind_table_by_num_hash::<DB, tables::CumulativeTxCount>(tx, input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
