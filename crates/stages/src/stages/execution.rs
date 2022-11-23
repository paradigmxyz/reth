use crate::{
    util::unwind::unwind_table_by_num_hash, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_interfaces::db::{
    models::BlockNumHash, tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut,
};
use std::fmt::Debug;

const TX_INDEX: StageId = StageId("Execution");

/// The execution stage executes all transactions and
/// update history indexes.
///
/// Input:
/// [tables::CanonicalHeaders] get next block to execute.
/// [tables::Headers] get for env
/// [tables::BlockBodies] to get tx number
/// [tables::Transactions] to execute
///
/// [StateProvider] needed for execution on most recent state:
/// [tables::PlainAccountState]
/// [tables::Bytecodes]
/// [tables::PlainStorageState]
///
/// [StateProvider] needed for execution on history state (Not needed for stage):
/// [tables::AccountHistory]
/// [tables::Bytecodes]
/// [tables::StorageHistory]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
///
/// Output:
/// [tables::PlainAccountState]
/// [tables::PlainStorageState]
/// [tables::Bytecodes]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
/// [tables::AccountHistory] and [tables::StorageHistory] indexes can be updated later in next stage?
///
/// For unwinds:
/// [tables::BlockBodies] get tx index to know what needs to be unwinded
/// [tables::AccountHistory] remove change set and apply old values to [tables::PlainAccountState]
/// [tables::StorageHistory] remove change set and apply old values to [tables::PlainStorageState]

#[derive(Debug)]
pub struct Execution;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for Execution {
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
        let db_tx = db.get_mut();
        let last_block = input.stage_progress.unwrap_or_default();
        let start_block = last_block + 1;

        // get inputs
        // [tables::CanonicalHeaders] get next block to execute.
        // [tables::Headers] get for env
        // [tables::BlockBodies] to get tx number
        // [tables::Transactions] to execute
        let mut cheaders = db_tx.cursor::<tables::CanonicalHeaders>()?;
        let mut headers = db_tx.cursor::<tables::Headers>()?;
        let mut bodies = db_tx.cursor::<tables::BlockBodies>()?;
        let mut tx = db_tx.cursor::<tables::Transactions>()?;
        let mut tx_sender = db_tx.cursor::<tables::TxSenders>()?;

        // load block ban of transactions and execute them

        // do N batches of block execution
        const BATCH_SIZE: u64 = 1000;

        let mut cheaders_batch = Vec::new();
        let index = start_block;
        let mut cheaders_walker = cheaders.walk(start_block)?;

        let mut end_block = start_block + BATCH_SIZE;
        // get canonical block (num,hash)
        for ch_index in start_block..end_block {
            if let Some(ch) = cheaders_walker.next() {
                cheaders_batch.push(BlockNumHash(ch?))
            } else {
                break;
            }
        }

        // get headers from canonical numbers
        let mut headers_batch = Vec::with_capacity(cheaders_batch.len());
        for ch_index in cheaders_batch.iter() {
            // TODO see if walker next has better performance then seek_exact calls.
            let (_, header) =
                headers.seek_exact(ch_index.clone())?.ok_or(DatabaseIntegrityError::Header {
                    number: ch_index.number(),
                    hash: ch_index.hash(),
                })?;
            headers_batch.push(header);
        }

        // get block
        let mut body_batch = Vec::with_capacity(cheaders_batch.len());
        for ch_index in cheaders_batch.iter() {
            let (_, block) = bodies
                .seek_exact(ch_index.clone())?
                .ok_or(DatabaseIntegrityError::BlockBody { number: ch_index.number() })?;
                body_batch.push(block);
        }

        for (header,body) in headers_batch.iter().zip(body_batch.iter()) {
            let start_tx_index = body.base_tx_id;
            let end_tx_index = body.tx_amount + start_tx_index;
            // iterate over all transactions
            let mut tx_walker = tx.walk(start_tx_index)?;
            let mut block_tx = Vec::with_capacity(body.tx_amount as usize);
            // get next N transactions.
            for index in start_tx_index..end_tx_index {
                let (tx_index, tx) =
                    tx_walker.next().ok_or(DatabaseIntegrityError::EndOfTransactionTable)??;
                if tx_index != index {
                    return Err(
                        DatabaseIntegrityError::TransactionsGap { missing: tx_index }.into()
                    );
                }
                block_tx.push(tx);
            }

            // EXECUTE TX
        }

        Ok(ExecOutput { done: true, reached_tip: true, stage_progress: last_block })
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

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_utils::{StageTestDB, StageTestRunner};
    use assert_matches::assert_matches;
    use reth_interfaces::{db::models::BlockNumHash, test_utils::generators::random_header_range};
    use reth_primitives::H256;

    const TEST_STAGE: StageId = StageId("PrevStage");

    #[tokio::test]
    async fn execute_empty_db() {
        let runner = TxIndexTestRunner::default();
        let rx = runner.execute(ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::CanonicalHeader { .. }))
        );
    }

    #[tokio::test]
    async fn execute_no_prev_tx_count() {
        let runner = TxIndexTestRunner::default();
        let headers = random_header_range(0..10, H256::zero());
        runner
            .db()
            .map_put::<tables::CanonicalHeaders, _, _>(&headers, |h| (h.number, h.hash()))
            .expect("failed to insert");

        let (head, tail) = (headers.first().unwrap(), headers.last().unwrap());
        let input = ExecInput {
            previous_stage: Some((TEST_STAGE, tail.number)),
            stage_progress: Some(head.number),
        };
        let rx = runner.execute(input);
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::CumulativeTxCount { .. }))
        );
    }

    #[tokio::test]
    async fn execute() {
        let runner = TxIndexTestRunner::default();
        let (start, pivot, end) = (0, 100, 200);
        let headers = random_header_range(start..end, H256::zero());
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
        let rx = runner.execute(input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress, done, reached_tip })
                if done && reached_tip && stage_progress == tail.number
        );
    }

    #[tokio::test]
    async fn unwind_empty_db() {
        let runner = TxIndexTestRunner::default();
        let rx = runner.unwind(UnwindInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == 0
        );
    }

    #[tokio::test]
    async fn unwind_no_input() {
        let runner = TxIndexTestRunner::default();
        let headers = random_header_range(0..10, H256::zero());
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
        let first_range = random_header_range(0..20, H256::zero());
        let second_range = random_header_range(50..100, H256::zero());
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
}
 */
