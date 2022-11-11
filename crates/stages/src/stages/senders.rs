use crate::{
    util::unwind::unwind_table_by_num, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use reth_interfaces::db::{
    self, tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut,
};
use reth_primitives::TxNumber;
use std::fmt::Debug;
use thiserror::Error;

const SENDERS: StageId = StageId("Senders");

#[derive(Debug)]
struct SendersStage {}

#[derive(Error, Debug)]
enum SendersStageError {
    #[error("Sender recovery failed for transaction {tx}.")]
    SenderRecovery { tx: TxNumber },
}

impl Into<StageError> for SendersStageError {
    fn into(self) -> StageError {
        StageError::Internal(Box::new(self))
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for SendersStage {
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
        let last_block_num = input.stage_progress.unwrap_or_default();
        let start_block = last_block_num + 1;
        let start_hash = tx
            .get::<tables::CanonicalHeaders>(start_block)?
            .ok_or(DatabaseIntegrityError::CannonicalHash { number: start_block })?;

        let max_block = input.previous_stage.map(|(_, num)| num).unwrap_or_default();
        let mut body_cursor = tx.cursor_mut::<tables::BlockBodies>()?;
        let mut body_walker =
            body_cursor.walk((start_block, start_hash).into())?.take_while(|res| {
                res.as_ref().map(|(k, _)| k.number() <= max_block).unwrap_or_default()
            });

        let mut senders_cursor = tx.cursor_mut::<tables::TxSenders>()?;
        let mut stage_progress = last_block_num;
        while let Some((key, tx_count)) = body_walker.next().transpose()? {
            stage_progress = key.number();
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
                .collect::<Result<Vec<_>, db::Error>>()?;

            for (id, transaction) in transactions.iter() {
                let signer = transaction
                    .recover_signer()
                    .ok_or::<StageError>(SendersStageError::SenderRecovery { tx: *id }.into())?;
                senders_cursor.append(*id, signer)?;
            }
        }

        Ok(ExecOutput { stage_progress, done: true, reached_tip: true })
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
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_utils::{StageTestDB, StageTestRunner, PREV_STAGE_ID};
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_interfaces::test_utils::{gen_random_header, gen_random_tx};
    use reth_primitives::H256;

    #[tokio::test]
    async fn execute_empty_db() {
        let runner = SendersTestRunner::default();
        let rx = runner.execute(ExecInput::default());
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::DatabaseIntegrity(DatabaseIntegrityError::CannonicalHash { .. }))
        );
    }

    #[tokio::test]
    async fn execute_no_bodies_no_progress() {
        let runner = SendersTestRunner::default();
        let stage_progress = gen_random_header(1, None);
        let prev_stage_progress = gen_random_header(100, None);
        runner
            .db()
            .map_put::<tables::CanonicalHeaders, _, _>(
                &[&stage_progress, &prev_stage_progress],
                |h| (h.number, h.hash()),
            )
            .expect("failed to insert");

        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, prev_stage_progress.number)),
            stage_progress: None,
        };
        let rx = runner.execute(input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress, done, reached_tip })
                if done && reached_tip && stage_progress == 0
        );
    }

    #[tokio::test]
    async fn execute() {
        let runner = SendersTestRunner::default();
        let (stage_progress, prev_stage_progress) = (100, 120);
        let start_header = gen_random_header(stage_progress + 1, None);
        runner
            .db()
            .put::<tables::CanonicalHeaders>(start_header.number, start_header.hash())
            .expect("failed to insert");

        let num_of_txs = 2000;
        let mut transactions =
            (0..num_of_txs).map(|_| gen_random_tx()).enumerate().collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut end_block = stage_progress; // start at start_header.number - 1
        let mut cum_tx_count = 0_u64;
        while !transactions.is_empty() {
            let block_tx_count = if transactions.len() > 5 {
                rng.gen_range(0..transactions.len() / 2)
            } else {
                transactions.len()
            };
            let block_txs = transactions.drain(..block_tx_count).collect::<Vec<_>>();
            runner
                .db()
                .map_put::<tables::Transactions, _, _>(&block_txs, |(idx, tx)| {
                    (*idx as u64, tx.clone())
                })
                .expect("failed to insert");

            end_block += 1;
            cum_tx_count += block_txs.len() as u64;
            let current_block_key = (end_block, H256::zero()).into();
            runner
                .db()
                .put::<tables::BlockBodies>(current_block_key, block_txs.len() as u16)
                .expect("failed to insert");
            runner
                .db()
                .put::<tables::CumulativeTxCount>(current_block_key, cum_tx_count)
                .expect("failed to insert");
        }

        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, prev_stage_progress)),
            stage_progress: Some(stage_progress),
        };
        let rx = runner.execute(input);
        let expected_progress = std::cmp::min(end_block, prev_stage_progress);
        assert_matches!(
            rx.await.unwrap(),
            Ok(ExecOutput { stage_progress, done, reached_tip })
                if done && reached_tip && stage_progress == expected_progress
        )
    }

    #[tokio::test]
    async fn unwind_empty_db() {
        let runner = SendersTestRunner::default();
        let rx = runner.unwind(UnwindInput::default());
        let result = rx.await.unwrap();
        assert!(result.is_err());
        let result = result.unwrap_err();
        assert_matches!(
            result.downcast_ref::<DatabaseIntegrityError>(),
            Some(DatabaseIntegrityError::CannonicalHeader { number })
                if *number == 0
        );
    }

    #[tokio::test]
    async fn unwind() {
        let runner = SendersTestRunner::default();

        let unwind_block = 100;
        // one transaction per block
        let transactions = (0..1000)
            .map(|idx| (idx, gen_random_tx().recover_signer().unwrap()))
            .collect::<Vec<_>>();

        runner
            .db()
            .map_put::<tables::TxSenders, _, _>(&transactions, |(k, v)| (*k, *v))
            .expect("failed to insert");

        // put one transaction at unwind block
        let unwind_tx_index = unwind_block + 1;
        runner
            .db()
            .put::<tables::CumulativeTxCount>((unwind_block, H256::zero()).into(), unwind_tx_index)
            .expect("failed to insert");
        runner
            .db()
            .put::<tables::CanonicalHeaders>(unwind_block, H256::zero())
            .expect("failed to insert");

        let input = UnwindInput { bad_block: None, stage_progress: 0, unwind_to: unwind_block };
        let rx = runner.unwind(input);
        assert_matches!(
            rx.await.unwrap(),
            Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_block
        );
        runner
            .db()
            .check_no_entry_above::<tables::TxSenders, _>(unwind_tx_index - 1, |key| key)
            .expect("failed to check");
    }

    #[derive(Default)]
    struct SendersTestRunner {
        db: StageTestDB,
    }

    impl StageTestRunner for SendersTestRunner {
        type S = SendersStage;

        fn db(&self) -> &StageTestDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            SendersStage {}
        }
    }
}
