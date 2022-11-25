use crate::{
    util::unwind::unwind_table_by_num_hash, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_interfaces::db::{tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut};
use std::fmt::Debug;

const TX_INDEX: StageId = StageId("TxIndex");

/// The cumulative transaction index stage
/// implementation for staged sync. This stage
/// updates the cumulative transaction count per block.
///
/// e.g. [key, value] entries in [tables::CumulativeTxCount]
/// block #1 with 24 transactions - [1, 24]
/// block #2 with 42 transactions - [2, 66]
/// block #3 with 33 transaction  - [3, 99]
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

        // The progress of this stage during last iteration
        let last_block = input.stage_progress.unwrap_or_default();
        let last_hash = tx
            .get::<tables::CanonicalHeaders>(last_block)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: last_block })?;

        // The start block for this iteration
        let start_block = last_block + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: start_block })?;

        // The maximum block that this stage should insert to
        let max_block = input.previous_stage_progress();

        // Get the cursor over the table
        let mut cursor = tx.cursor_mut::<tables::CumulativeTxCount>()?;
        // Find the last count that was inserted during previous iteration
        let (_, mut count) = cursor.seek_exact((last_block, last_hash).into())?.ok_or(
            DatabaseIntegrityError::CumulativeTxCount { number: last_block, hash: last_hash },
        )?;

        // Get the cursor over block bodies
        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let walker = body_cursor.walk((start_block, start_hash).into())?;

        // Walk the block body entries up to maximum block (including)
        let entries = walker
            .take_while(|b| b.as_ref().map(|(k, _)| k.number() <= max_block).unwrap_or_default());

        // Aggregate and insert cumulative transaction count for each block number
        for entry in entries {
            let (key, body) = entry?;
            count += body.tx_amount;
            cursor.append(key, count)?;
        }

        Ok(ExecOutput { done: true, reached_tip: true, stage_progress: max_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        unwind_table_by_num_hash::<DB, tables::CumulativeTxCount>(db.get_mut(), input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestDB, StageTestRunner, TestRunnerError,
        UnwindStageTestRunner,
    };
    use reth_interfaces::{
        db::models::{BlockNumHash, StoredBlockBody},
        test_utils::generators::random_header_range,
    };
    use reth_primitives::H256;

    stage_test_suite!(TxIndexTestRunner);

    #[derive(Default)]
    pub(crate) struct TxIndexTestRunner {
        db: StageTestDB,
    }

    impl StageTestRunner for TxIndexTestRunner {
        type S = TxIndex;

        fn db(&self) -> &StageTestDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            TxIndex {}
        }
    }

    impl ExecuteStageTestRunner for TxIndexTestRunner {
        type Seed = ();

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let pivot = input.stage_progress.unwrap_or_default();
            let start = pivot.saturating_sub(100);
            let mut end = input.previous_stage_progress();
            end += 2; // generate 2 additional headers to account for start header lookup
            let headers = random_header_range(start..end, H256::zero());

            let headers =
                headers.into_iter().map(|h| (h, rand::random::<u8>())).collect::<Vec<_>>();

            self.db.map_put::<tables::CanonicalHeaders, _, _>(&headers, |(h, _)| {
                (h.number, h.hash())
            })?;
            self.db.map_put::<tables::BlockBodies, _, _>(&headers, |(h, count)| {
                (
                    BlockNumHash((h.number, h.hash())),
                    StoredBlockBody { base_tx_id: 0, tx_amount: *count as u64, ommers: vec![] },
                )
            })?;

            let slice_up_to =
                std::cmp::min(pivot.saturating_sub(start) as usize, headers.len() - 1);
            self.db.transform_append::<tables::CumulativeTxCount, _, _>(
                &headers[..=slice_up_to],
                |prev, (h, count)| {
                    (BlockNumHash((h.number, h.hash())), prev.unwrap_or_default() + (*count as u64))
                },
            )?;

            Ok(())
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            _output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            // TODO: validate that base_tx_index of next block equals the cum count at current
            self.db.query(|tx| {
                let (start, end) =
                    (input.stage_progress.unwrap_or_default(), input.previous_stage_progress());
                if start >= end {
                    return Ok(())
                }

                let start_hash =
                    tx.get::<tables::CanonicalHeaders>(start)?.expect("no canonical found");
                let mut tx_count_cursor = tx.cursor::<tables::CumulativeTxCount>()?;
                let mut tx_count_walker = tx_count_cursor.walk((start, start_hash).into())?;
                let mut count = tx_count_walker.next().unwrap()?.1;
                let mut last_num = start;
                for entry in tx_count_walker {
                    let (key, db_count) = entry?;
                    count += tx.get::<tables::BlockBodies>(key)?.unwrap().tx_amount;
                    assert_eq!(db_count, count);
                    last_num = key.number();
                }
                assert_eq!(last_num, end);

                Ok(())
            })?;
            Ok(())
        }
    }

    impl UnwindStageTestRunner for TxIndexTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.db.check_no_entry_above::<tables::CumulativeTxCount, _>(input.unwind_to, |h| {
                h.number()
            })?;
            Ok(())
        }
    }
}
