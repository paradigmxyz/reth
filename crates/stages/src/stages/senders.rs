use crate::{
    db::StageDB, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use rayon::prelude::*;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::TxNumber;
use std::fmt::Debug;
use thiserror::Error;

const SENDERS: StageId = StageId("Senders");

/// The senders stage iterates over existing transactions,
/// recovers the transaction signer and stores them
/// in [`TxSenders`][reth_interfaces::db::tables::TxSenders] table.
#[derive(Debug)]
pub struct SendersStage {
    /// The size of the chunk for parallel sender recovery
    pub batch_size: usize,
}

#[derive(Error, Debug)]
enum SendersStageError {
    #[error("Sender recovery failed for transaction {tx}.")]
    SenderRecovery { tx: TxNumber },
}

impl From<SendersStageError> for StageError {
    fn from(error: SendersStageError) -> Self {
        StageError::Internal(Box::new(error))
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for SendersStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        SENDERS
    }

    /// Retrieve the range of transactions to iterate over by querying
    /// [`CumulativeTxCount`][reth_interfaces::db::tables::CumulativeTxCount],
    /// collect transactions within that range,
    /// recover signer for each transaction and store entries in
    /// the [`TxSenders`][reth_interfaces::db::tables::TxSenders] table.
    async fn execute(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();

        // Look up the start index for the transaction range
        let start_tx_index = db.get_first_tx_id(stage_progress + 1)?;

        // Look up the end index for transaction range (inclusive)
        let max_block_num = input.previous_stage_progress();
        let end_tx_index = match db.get_latest_tx_id(max_block_num) {
            Ok(id) => id,
            // No transactions in the database
            Err(_) => {
                return Ok(ExecOutput {
                    stage_progress: max_block_num,
                    done: true,
                    reached_tip: true,
                })
            }
        };

        // Acquire the cursor for inserting elements
        let mut senders_cursor = db.cursor_mut::<tables::TxSenders>()?;

        // Acquire the cursor over the transactions
        let mut tx_cursor = db.cursor::<tables::Transactions>()?;
        // Walk the transactions from start to end index (inclusive)
        let entries = tx_cursor
            .walk(start_tx_index)?
            .take_while(|res| res.as_ref().map(|(k, _)| *k <= end_tx_index).unwrap_or_default());

        // Iterate over transactions in chunks
        for chunk in &entries.chunks(self.batch_size) {
            let transactions = chunk.collect::<Result<Vec<_>, DbError>>()?;
            // Recover signers for the chunk in parallel
            let recovered = transactions
                .into_par_iter()
                .map(|(id, transaction)| {
                    let signer =
                        transaction.recover_signer().ok_or_else::<StageError, _>(|| {
                            SendersStageError::SenderRecovery { tx: id }.into()
                        })?;
                    Ok((id, signer))
                })
                .collect::<Result<Vec<_>, StageError>>()?;
            // Append the signers to the table
            recovered.into_iter().try_for_each(|(id, sender)| senders_cursor.append(id, sender))?;
        }

        Ok(ExecOutput { stage_progress: max_block_num, done: true, reached_tip: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        // Lookup latest tx id that we should unwind to
        let latest_tx_id = db.get_latest_tx_id(input.unwind_to).unwrap_or_default();
        db.unwind_table_by_num::<tables::TxSenders>(latest_tx_id)?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{BlockLocked, BlockNumber, H256};

    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestStageDB,
        UnwindStageTestRunner,
    };

    stage_test_suite!(SendersTestRunner);

    #[derive(Default)]
    struct SendersTestRunner {
        db: TestStageDB,
    }

    impl StageTestRunner for SendersTestRunner {
        type S = SendersStage;

        fn db(&self) -> &TestStageDB {
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
                    let txs = b.body.clone();
                    let tx_amount = txs.len() as u64;

                    let num_hash = (b.number, b.hash()).into();
                    tx.put::<tables::CanonicalHeaders>(b.number, b.hash())?;
                    tx.put::<tables::CumulativeTxCount>(num_hash, base_tx_id + tx_amount)?;

                    for body_tx in txs {
                        // Insert senders for previous stage progress
                        if b.number == stage_progress {
                            tx.put::<tables::TxSenders>(
                                base_tx_id,
                                body_tx.recover_signer().expect("failed to recover sender"),
                            )?;
                        }
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
                    let end_block = output.stage_progress;

                    if start_block > end_block {
                        return Ok(())
                    }

                    let mut tx_count_cursor = tx.cursor::<tables::CumulativeTxCount>()?;

                    let last_block = start_block - 1;
                    let last_hash = tx.get::<tables::CanonicalHeaders>(start_block)?.unwrap();
                    let mut last_tx_count = tx_count_cursor
                        .seek_exact((last_block, last_hash).into())?
                        .map(|(_, v)| v)
                        .unwrap_or_default();

                    while let Some((_, count)) = tx_count_cursor.next()? {
                        for tx_id in last_tx_count..count {
                            let transaction = tx
                                .get::<tables::Transactions>(tx_id)?
                                .expect("no transaction entry");
                            let signer =
                                transaction.recover_signer().expect("failed to recover signer");
                            assert_eq!(Some(signer), tx.get::<tables::TxSenders>(tx_id)?);
                        }
                        last_tx_count = count;
                    }

                    Ok(())
                })?;
            } else {
                self.check_no_senders_by_block(input.stage_progress.unwrap_or_default())?;
            }

            Ok(())
        }
    }

    impl UnwindStageTestRunner for SendersTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.check_no_senders_by_block(input.unwind_to)
        }
    }

    impl SendersTestRunner {
        fn check_no_senders_by_block(&self, block: BlockNumber) -> Result<(), TestRunnerError> {
            let latest_tx_id = self.db.inner().get_latest_tx_id(block);
            match latest_tx_id {
                Ok(last_index) => {
                    self.db.check_no_entry_above::<tables::TxSenders, _>(last_index, |key| key)?
                }
                Err(_) => {
                    assert!(self.db.table_is_empty::<tables::TxSenders>()?);
                }
            };

            Ok(())
        }
    }
}
