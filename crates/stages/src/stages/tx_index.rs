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
            .ok_or(DatabaseIntegrityError::CannonicalHeader { number: last_block })?;

        // The start block for this iteration
        let start_block = last_block + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or(DatabaseIntegrityError::CannonicalHeader { number: start_block })?;

        // The maximum block that this stage should insert to
        let max_block = input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();

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
            let (key, tx_count) = entry?;
            count += tx_count as u64;
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
    use crate::util::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestDB, StageTestRunner,
        UnwindStageTestRunner, PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::{db::models::BlockNumHash, test_utils::gen_random_header_range};
    use reth_primitives::H256;

    stage_test_suite!(TxIndexTestRunner);

    #[tokio::test]
    async fn execute_no_prev_tx_count() {
        let runner = TxIndexTestRunner::default();
        let headers = gen_random_header_range(0..10, H256::zero());
        runner
            .db()
            .map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| (h.number, h.hash()))
            .expect("failed to insert");

        let (head, tail) = (headers.first().unwrap(), headers.last().unwrap());
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, tail.number)),
            stage_progress: Some(head.number),
        };
        let rx = runner.execute(input);
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::CumulativeTxCount { .. }))
        );
    }

    #[tokio::test]
    async fn unwind_no_input() {
        let runner = TxIndexTestRunner::default();
        let headers = gen_random_header_range(0..10, H256::zero());
        runner
            .db()
            .transform_append::<tables::CumulativeTxCount, _, _>(&headers, |prev, h| {
                (
                    BlockNumHash((h.number, h.hash())),
                    prev.unwrap_or_default() + (rand::random::<u8>() as u64),
                )
            })
            .expect("failed to insert");

        let rx = runner.unwind(UnwindInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == 0
        );
        runner
            .db()
            .check_no_entry_above::<tables::CumulativeTxCount, _>(0, |h| h.number())
            .expect("failed to check tx count");
    }

    #[tokio::test]
    async fn unwind_with_db_gaps() {
        let runner = TxIndexTestRunner::default();
        let first_range = gen_random_header_range(0..20, H256::zero());
        let second_range = gen_random_header_range(50..100, H256::zero());
        runner
            .db()
            .transform_append::<tables::CumulativeTxCount, _, _>(
                &first_range.iter().chain(second_range.iter()).collect::<Vec<_>>(),
                |prev, h| {
                    (
                        BlockNumHash((h.number, h.hash())),
                        prev.unwrap_or_default() + (rand::random::<u8>() as u64),
                    )
                },
            )
            .expect("failed to insert");

        let unwind_to = 10;
        let input = UnwindInput { unwind_to, ..Default::default() };
        let rx = runner.unwind(input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_to
        );
        runner
            .db()
            .check_no_entry_above::<tables::CumulativeTxCount, _>(unwind_to, |h| h.number())
            .expect("failed to check tx count");
    }

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
        fn seed_execution(
            &mut self,
            input: ExecInput,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let pivot = input.stage_progress.unwrap_or_default();
            let start = pivot.saturating_sub(100);
            let end = input.previous_stage.as_ref().map(|(_, num)| *num).unwrap_or_default();
            let headers = gen_random_header_range(start..end, H256::zero());
            self.db()
                .map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| (h.number, h.hash()))?;
            self.db().transform_append::<tables::CumulativeTxCount, _, _>(
                &headers[..=(pivot as usize)],
                |prev, h| {
                    (
                        BlockNumHash((h.number, h.hash())),
                        prev.unwrap_or_default() + (rand::random::<u8>() as u64),
                    )
                },
            )?;
            Ok(())
        }

        fn validate_execution(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            // TODO:
            Ok(())
        }
    }

    impl UnwindStageTestRunner for TxIndexTestRunner {
        fn seed_unwind(
            &mut self,
            input: UnwindInput,
            highest_entry: u64,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            // TODO: accept range
            let headers = gen_random_header_range(input.unwind_to..highest_entry, H256::zero());
            self.db().transform_append::<tables::CumulativeTxCount, _, _>(
                &headers,
                |prev, h| {
                    (
                        BlockNumHash((h.number, h.hash())),
                        prev.unwrap_or_default() + (rand::random::<u8>() as u64),
                    )
                },
            )?;
            Ok(())
        }

        fn validate_unwind(
            &self,
            input: UnwindInput,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.db()
                .check_no_entry_above::<tables::CumulativeTxCount, _>(input.unwind_to, |h| {
                    h.number()
                })?;
            Ok(())
        }
    }
}
