use crate::{
    util::unwind::unwind_table_by_num_hash, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_interfaces::db::{tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut};
use std::fmt::Debug;

const TX_INDEX: StageId = StageId("TxIndex");

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
            .ok_or(DatabaseIntegrityError::NoCannonicalHeader { number: last_block })?;

        let start_block = last_block + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or(DatabaseIntegrityError::NoCannonicalHeader { number: start_block })?;

        let max_block = input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();

        let mut cursor = tx.cursor_mut::<tables::CumulativeTxCount>()?;
        let (_, mut count) = cursor.seek_exact((last_block, last_hash).into())?.ok_or(
            DatabaseIntegrityError::NoCumulativeTxCount { number: last_block, hash: last_hash },
        )?;

        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let walker = body_cursor.walk((start_block, start_hash).into())?;
        let entries = walker
            .take_while(|b| b.as_ref().map(|(k, _)| k.number() <= max_block).unwrap_or_default());
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
        let tx = db.get_mut();
        unwind_table_by_num_hash::<DB, tables::CumulativeTxCount>(tx, input.unwind_to)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_utils::{StageTestDB, StageTestRunner};
    use assert_matches::assert_matches;
    use reth_interfaces::{db::models::BlockNumHash, test_utils::gen_random_header_range};
    use reth_primitives::H256;

    const TEST_STAGE: StageId = StageId("PrevStage");

    #[tokio::test]
    async fn execute_empty_db() {
        let runner = TxIndexTestRunner::default();
        let rx = runner.execute(TxIndex {}, ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::NoCannonicalHeader { .. }))
        );
    }

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
            previous_stage: Some((TEST_STAGE, tail.number)),
            stage_progress: Some(head.number),
        };
        let rx = runner.execute(TxIndex {}, input);
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::NoCumulativeTxCount { .. }))
        );
    }

    #[tokio::test]
    async fn execute() {
        let runner = TxIndexTestRunner::default();
        let (start, pivot, end) = (0, 100, 200);
        let headers = gen_random_header_range(start..end, H256::zero());
        runner
            .db()
            .map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| (h.number, h.hash()))
            .expect("failed to insert");
        runner
            .db()
            .transform_append::<tables::CumulativeTxCount, _, _>(&headers[..=pivot], |prev, h| {
                (
                    BlockNumHash((h.number, h.hash())),
                    prev.unwrap_or_default() + (rand::random::<u8>() as u64),
                )
            })
            .expect("failed to insert");

        let (pivot, tail) = (headers.get(pivot).unwrap(), headers.last().unwrap());
        let input = ExecInput {
            previous_stage: Some((TEST_STAGE, tail.number)),
            stage_progress: Some(pivot.number),
        };
        let rx = runner.execute(TxIndex {}, input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress, done, reached_tip })
                if done && reached_tip && stage_progress == tail.number
        );
    }

    #[tokio::test]
    async fn unwind_empty_db() {
        let runner = TxIndexTestRunner::default();
        let rx = runner.unwind(TxIndex {}, UnwindInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == 0
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

        let rx = runner.unwind(TxIndex {}, UnwindInput::default());
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
        let rx = runner.unwind(TxIndex {}, input);
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
        fn db(&self) -> &StageTestDB {
            &self.db
        }
    }
}
