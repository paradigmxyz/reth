use crate::{
    util::unwind::unwind_table_by_num, DatabaseIntegrityError, ExecInput, ExecOutput, Stage,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use rayon::prelude::*;
use reth_interfaces::db::{
    self, tables, DBContainer, Database, DbCursorRO, DbCursorRW, DbTx, DbTxMut,
};
use reth_primitives::{Address, TxNumber};
use std::fmt::Debug;
use thiserror::Error;

const SENDERS: StageId = StageId("Senders");

/// TODO: docs
#[derive(Debug)]
struct SendersStage {
    batch_size: usize,
}

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
        let last_block_hash = tx
            .get::<tables::CanonicalHeaders>(last_block_num)?
            .ok_or(DatabaseIntegrityError::CanonicalHash { number: last_block_num })?;

        let max_block_num = input.previous_stage_progress();
        let max_block_hash = tx
            .get::<tables::CanonicalHeaders>(max_block_num)?
            .ok_or(DatabaseIntegrityError::CanonicalHash { number: max_block_num })?;

        let start_tx_index = tx
            .get::<tables::CumulativeTxCount>((last_block_num, last_block_hash).into())?
            .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                number: last_block_num,
                hash: last_block_hash,
            })?;
        let end_tx_index = tx
            .get::<tables::CumulativeTxCount>((max_block_num, max_block_hash).into())?
            .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                number: last_block_num,
                hash: last_block_hash,
            })?;

        let mut senders_cursor = tx.cursor_mut::<tables::TxSenders>()?;
        let mut tx_cursor = tx.cursor::<tables::Transactions>()?;
        let entries = tx_cursor
            .walk(start_tx_index)?
            .take_while(|res| res.as_ref().map(|(k, _)| *k < end_tx_index).unwrap_or_default());

        for chunk in &entries.chunks(self.batch_size as usize) {
            let transactions = chunk.collect::<Result<Vec<_>, db::Error>>()?;
            let recovered = transactions
                .into_par_iter()
                .map(|(id, transaction)| {
                    let signer = transaction
                        .recover_signer()
                        .ok_or::<StageError>(SendersStageError::SenderRecovery { tx: id }.into())?;
                    Ok((id, signer))
                })
                .collect::<Result<Vec<_>, StageError>>()?;
            recovered.into_iter().try_for_each(|(id, sender)| senders_cursor.append(id, sender))?;
        }

        Ok(ExecOutput { stage_progress: max_block_num, done: true, reached_tip: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let tx = db.get_mut();

        // Look up the hash of the unwind block
        if let Some(unwind_hash) = tx.get::<tables::CanonicalHeaders>(input.unwind_to)? {
            // Look up the cumulative tx count at unwind block
            let latest_tx = tx
                .get::<tables::CumulativeTxCount>((input.unwind_to, unwind_hash).into())?
                .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                    number: input.unwind_to,
                    hash: unwind_hash,
                })?;

            unwind_table_by_num::<DB, tables::TxSenders>(tx, latest_tx - 1)?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use reth_interfaces::{
        db::models::StoredBlockBody, test_utils::generators::random_block_range,
    };
    use reth_primitives::{BlockLocked, BlockNumber, H256};

    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestDB, StageTestRunner, TestRunnerError,
        UnwindStageTestRunner,
    };

    stage_test_suite!(SendersTestRunner);

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
            SendersStage { batch_size: 100 }
        }
    }

    impl ExecuteStageTestRunner for SendersTestRunner {
        type Seed = Vec<BlockLocked>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress() + 1;

            let blocks = random_block_range(stage_progress..end, H256::zero());

            self.db.commit(|tx| {
                let mut base_tx_id = 0;
                blocks.iter().try_for_each(|b| {
                    let ommers = b.ommers.iter().map(|o| o.clone().unseal()).collect::<Vec<_>>();
                    let txs = b.body.clone();
                    let tx_amount = txs.len() as u64;

                    let num_hash = (b.number, b.hash()).into();
                    tx.put::<tables::CanonicalHeaders>(b.number, b.hash())?;
                    tx.put::<tables::BlockBodies>(
                        num_hash,
                        StoredBlockBody { base_tx_id, tx_amount, ommers },
                    )?;
                    tx.put::<tables::CumulativeTxCount>(num_hash, base_tx_id + tx_amount)?;

                    for body_tx in txs {
                        tx.put::<tables::Transactions>(base_tx_id, body_tx)?;
                        base_tx_id += 1;
                    }

                    Ok(())
                })?;
                Ok(())
            })?;

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                self.db.query(|tx| {
                    let start_block = input.stage_progress.unwrap_or_default() + 1;
                    let end_block = input.previous_stage_progress();

                    if start_block > end_block {
                        return Ok(())
                    }

                    let start_hash = tx.get::<tables::CanonicalHeaders>(start_block)?.unwrap();
                    let mut body_cursor = tx.cursor::<tables::BlockBodies>()?;
                    let mut body_walker = body_cursor.walk((start_block, start_hash).into())?;

                    while let Some(entry) = body_walker.next() {
                        let (_, body) = entry?;
                        for tx_id in body.base_tx_id..body.base_tx_id + body.tx_amount {
                            let transaction = tx
                                .get::<tables::Transactions>(tx_id)?
                                .expect("no transaction entry");
                            let signer =
                                transaction.recover_signer().expect("failed to recover signer");
                            assert_eq!(Some(signer), tx.get::<tables::TxSenders>(tx_id)?);
                        }
                    }

                    Ok(())
                })?;
            }
            // TODO:
            //  else {
            // }

            Ok(())
        }
    }

    impl UnwindStageTestRunner for SendersTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            match self.get_block_body_entry(input.unwind_to)? {
                Some(body) => {
                    let last_index = body.base_tx_id + body.tx_amount;
                    self.db.check_no_entry_above::<tables::TxSenders, _>(last_index, |key| key)?;
                }
                None => {
                    assert!(self.db.table_is_empty::<tables::TxSenders>()?);
                }
            }

            Ok(())
        }
    }

    impl SendersTestRunner {
        fn get_block_body_entry(
            &self,
            block: BlockNumber,
        ) -> Result<Option<StoredBlockBody>, TestRunnerError> {
            let entry = self.db.query(|tx| {
                let body = tx
                    .get::<tables::CanonicalHeaders>(block)?
                    .map(|hash| tx.get::<tables::BlockBodies>((block, hash).into()));
                Ok(body.transpose()?.flatten())
            })?;
            Ok(entry)
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::test_utils::{StageTestDB, StageTestRunner, PREV_STAGE_ID};
//     use assert_matches::assert_matches;
//     use rand::Rng;
//     use reth_interfaces::test_utils::{gen_random_header, gen_random_tx};
//     use reth_primitives::H256;

//     #[tokio::test]
//     async fn execute_no_bodies_no_progress() {
//         let runner = SendersTestRunner::default();
//         let stage_progress = gen_random_header(1, None);
//         let prev_stage_progress = gen_random_header(100, None);
//         runner
//             .db()
//             .map_put::<tables::CanonicalHeaders, _, _>(
//                 &[&stage_progress, &prev_stage_progress],
//                 |h| (h.number, h.hash()),
//             )
//             .expect("failed to insert");

//         let input = ExecInput {
//             previous_stage: Some((PREV_STAGE_ID, prev_stage_progress.number)),
//             stage_progress: None,
//         };
//         let rx = runner.execute(input);
//         assert_matches!(
//             rx.await.unwrap(),
//             Ok(ExecOutput { stage_progress, done, reached_tip })
//                 if done && reached_tip && stage_progress == 0
//         );
//     }

//     #[tokio::test]
//     async fn execute() {
//         let runner = SendersTestRunner::default();
//         let (stage_progress, prev_stage_progress) = (100, 120);
//         let start_header = gen_random_header(stage_progress + 1, None);
//         runner
//             .db()
//             .put::<tables::CanonicalHeaders>(start_header.number, start_header.hash())
//             .expect("failed to insert");

//         let num_of_txs = 2000;
//         let mut transactions =
//             (0..num_of_txs).map(|_| gen_random_tx()).enumerate().collect::<Vec<_>>();

//         let mut rng = rand::thread_rng();
//         let mut end_block = stage_progress; // start at start_header.number - 1
//         let mut cum_tx_count = 0_u64;
//         while !transactions.is_empty() {
//             let block_tx_count = if transactions.len() > 5 {
//                 rng.gen_range(0..transactions.len() / 2)
//             } else {
//                 transactions.len()
//             };
//             let block_txs = transactions.drain(..block_tx_count).collect::<Vec<_>>();
//             runner
//                 .db()
//                 .map_put::<tables::Transactions, _, _>(&block_txs, |(idx, tx)| {
//                     (*idx as u64, tx.clone())
//                 })
//                 .expect("failed to insert");

//             end_block += 1;
//             cum_tx_count += block_txs.len() as u64;
//             let current_block_key = (end_block, H256::zero()).into();
//             runner
//                 .db()
//                 .put::<tables::BlockBodies>(current_block_key, block_txs.len() as u16)
//                 .expect("failed to insert");
//             runner
//                 .db()
//                 .put::<tables::CumulativeTxCount>(current_block_key, cum_tx_count)
//                 .expect("failed to insert");
//         }

//         let input = ExecInput {
//             previous_stage: Some((PREV_STAGE_ID, prev_stage_progress)),
//             stage_progress: Some(stage_progress),
//         };
//         let rx = runner.execute(input);
//         let expected_progress = std::cmp::min(end_block, prev_stage_progress);
//         assert_matches!(
//             rx.await.unwrap(),
//             Ok(ExecOutput { stage_progress, done, reached_tip })
//                 if done && reached_tip && stage_progress == expected_progress
//         )
//     }

//     #[tokio::test]
//     async fn unwind_empty_db() {
//         let runner = SendersTestRunner::default();
//         let rx = runner.unwind(UnwindInput::default());
//         let result = rx.await.unwrap();
//         assert!(result.is_err());
//         let result = result.unwrap_err();
//         assert_matches!(
//             result.downcast_ref::<DatabaseIntegrityError>(),
//             Some(DatabaseIntegrityError::CannonicalHeader { number })
//                 if *number == 0
//         );
//     }

//     #[tokio::test]
//     async fn unwind() {
//         let runner = SendersTestRunner::default();

//         let unwind_block = 100;
//         // one transaction per block
//         let transactions = (0..1000)
//             .map(|idx| (idx, gen_random_tx().recover_signer().unwrap()))
//             .collect::<Vec<_>>();

//         runner
//             .db()
//             .map_put::<tables::TxSenders, _, _>(&transactions, |(k, v)| (*k, *v))
//             .expect("failed to insert");

//         // put one transaction at unwind block
//         let unwind_tx_index = unwind_block + 1;
//         runner
//             .db()
//             .put::<tables::CumulativeTxCount>((unwind_block, H256::zero()).into(),
// unwind_tx_index)             .expect("failed to insert");
//         runner
//             .db()
//             .put::<tables::CanonicalHeaders>(unwind_block, H256::zero())
//             .expect("failed to insert");

//         let input = UnwindInput { bad_block: None, stage_progress: 0, unwind_to: unwind_block };
//         let rx = runner.unwind(input);
//         assert_matches!(
//             rx.await.unwrap(),
//             Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_block
//         );
//         runner
//             .db()
//             .check_no_entry_above::<tables::TxSenders, _>(unwind_tx_index - 1, |key| key)
//             .expect("failed to check");
//     }

//     #[derive(Default)]
//     struct SendersTestRunner {
//         db: StageTestDB,
//     }

//     impl StageTestRunner for SendersTestRunner {
//         type S = SendersStage;

//         fn db(&self) -> &StageTestDB {
//             &self.db
//         }

//         fn stage(&self) -> Self::S {
//             SendersStage {}
//         }
//     }
// }
