use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable, RawValue,
};
use reth_primitives::{keccak256, TransactionSignedNoHash, TxNumber, H160};
use reth_provider::Transaction;
use std::fmt::Debug;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::*;

/// The [`StageId`] of the sender recovery stage.
pub const SENDER_RECOVERY: StageId = StageId("SenderRecovery");

/// The sender recovery stage iterates over existing transactions,
/// recovers the transaction signer and stores them
/// in [`TxSenders`][reth_db::tables::TxSenders] table.
#[derive(Clone, Debug)]
pub struct SenderRecoveryStage {
    /// The size of inserted items after which the control
    /// flow will be returned to the pipeline for commit
    pub commit_threshold: u64,
}

impl Default for SenderRecoveryStage {
    fn default() -> Self {
        Self { commit_threshold: 500_000 }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for SenderRecoveryStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        SENDER_RECOVERY
    }

    /// Retrieve the range of transactions to iterate over by querying
    /// [`BlockBodyIndices`][reth_db::tables::BlockBodyIndices],
    /// collect transactions within that range,
    /// recover signer for each transaction and store entries in
    /// the [`TxSenders`][reth_db::tables::TxSenders] table.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);
        if range.is_empty() {
            return Ok(ExecOutput::done(*range.end()))
        }
        let (start_block, end_block) = range.clone().into_inner();

        // Look up the start index for the transaction range
        let first_tx_num = tx.block_body_indices(start_block)?.first_tx_num();

        // Look up the end index for transaction range (inclusive)
        let last_tx_num = tx.block_body_indices(end_block)?.last_tx_num();

        // No transactions to walk over
        if first_tx_num > last_tx_num {
            info!(target: "sync::stages::sender_recovery", first_tx_num, last_tx_num, "Target transaction already reached");
            return Ok(ExecOutput { stage_progress: end_block, done: is_final_range })
        }

        // Acquire the cursor for inserting elements
        let mut senders_cursor = tx.cursor_write::<tables::TxSenders>()?;

        // Acquire the cursor over the transactions
        let mut tx_cursor = tx.cursor_read::<RawTable<tables::Transactions>>()?;
        // Walk the transactions from start to end index (inclusive)
        let tx_walker =
            tx_cursor.walk_range(RawKey::new(first_tx_num)..=RawKey::new(last_tx_num))?;

        // Iterate over transactions in chunks
        info!(target: "sync::stages::sender_recovery", first_tx_num, last_tx_num, "Recovering senders");

        // channels used to return result of sender recovery.
        let mut channels = Vec::new();

        // Spawn recovery jobs onto the default rayon threadpool and send the result through the
        // channel.
        //
        // We try to evenly divide the transactions to recover across all threads in the threadpool.
        // Chunks are submitted instead of individual transactions to reduce the overhead of work
        // stealing in the threadpool workers.
        for chunk in
            &tx_walker.chunks(self.commit_threshold as usize / rayon::current_num_threads())
        {
            // An _unordered_ channel to receive results from a rayon job
            let (tx, rx) = mpsc::unbounded_channel();
            channels.push(rx);
            // Note: Unfortunate side-effect of how chunk is designed in itertools (it is not Send)
            let chunk: Vec<_> = chunk.collect();

            // closure that would recover signer. Used as utility to wrap result
            let recover = |entry: Result<
                (RawKey<TxNumber>, RawValue<TransactionSignedNoHash>),
                reth_db::DatabaseError,
            >,
                           rlp_buf: &mut Vec<u8>|
             -> Result<(u64, H160), Box<StageError>> {
                let (tx_id, transaction) = entry.map_err(|e| Box::new(e.into()))?;
                let tx_id = tx_id.key().expect("key to be formated");

                let tx = transaction.value().expect("value to be formated");
                tx.transaction.encode_without_signature(rlp_buf);

                let sender = tx.signature.recover_signer(keccak256(rlp_buf)).ok_or(
                    StageError::from(SenderRecoveryStageError::SenderRecovery { tx: tx_id }),
                )?;

                Ok((tx_id, sender))
            };

            // Spawn the sender recovery task onto the global rayon pool
            // This task will send the results through the channel after it recovered the senders.
            rayon::spawn(move || {
                let mut rlp_buf = Vec::with_capacity(128);
                for entry in chunk {
                    rlp_buf.clear();
                    let _ = tx.send(recover(entry, &mut rlp_buf));
                }
            });
        }
        // Iterate over channels and append the sender in the order that they are received.
        for mut channel in channels {
            while let Some(recovered) = channel.recv().await {
                let (tx_id, sender) = recovered.map_err(|boxed| *boxed)?;
                senders_cursor.append(tx_id, sender)?;
            }
        }

        info!(target: "sync::stages::sender_recovery", stage_progress = end_block, is_final_range, "Stage iteration finished");
        Ok(ExecOutput { stage_progress: end_block, done: is_final_range })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (_, unwind_to, is_final_range) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        // Lookup latest tx id that we should unwind to
        let latest_tx_id = tx.block_body_indices(unwind_to)?.last_tx_num();
        tx.unwind_table_by_num::<tables::TxSenders>(latest_tx_id)?;

        info!(target: "sync::stages::sender_recovery", to_block = input.unwind_to, unwind_progress = unwind_to, is_final_range, "Unwind iteration finished");
        Ok(UnwindOutput { stage_progress: unwind_to })
    }
}

// TODO(onbjerg): Should unwind
#[derive(Error, Debug)]
enum SenderRecoveryStageError {
    #[error("Sender recovery failed for transaction {tx}.")]
    SenderRecovery { tx: TxNumber },
}

impl From<SenderRecoveryStageError> for StageError {
    fn from(error: SenderRecoveryStageError) -> Self {
        StageError::Fatal(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::{random_block, random_block_range};
    use reth_primitives::{BlockNumber, SealedBlock, TransactionSigned, H256};

    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner, PREV_STAGE_ID,
    };

    stage_test_suite_ext!(SenderRecoveryTestRunner, sender_recovery);

    /// Execute a block range with a single transaction
    #[tokio::test]
    async fn execute_single_transaction() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let runner = SenderRecoveryTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        // Insert blocks with a single transaction at block `stage_progress + 10`
        let non_empty_block_number = stage_progress + 10;
        let blocks = (stage_progress..=input.previous_stage_progress())
            .map(|number| {
                random_block(number, None, Some((number == non_empty_block_number) as u8), None)
            })
            .collect::<Vec<_>>();
        runner.tx.insert_blocks(blocks.iter(), None).expect("failed to insert blocks");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done, stage_progress })
                if done && stage_progress == previous_stage
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    /// Execute the stage twice with input range that exceeds the commit threshold
    #[tokio::test]
    async fn execute_intermediate_commit() {
        let threshold = 50;
        let mut runner = SenderRecoveryTestRunner::default();
        runner.set_threshold(threshold);
        let (stage_progress, previous_stage) = (1000, 1100); // input exceeds threshold
        let first_input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        // Seed only once with full input range
        runner.seed_execution(first_input).expect("failed to seed execution");

        // Execute first time
        let result = runner.execute(first_input).await.unwrap();
        let expected_progress = stage_progress + threshold;
        assert_matches!(
            result,
            Ok(ExecOutput { done: false, stage_progress })
                if stage_progress == expected_progress
        );

        // Execute second time
        let second_input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(expected_progress),
        };
        let result = runner.execute(second_input).await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done: true, stage_progress })
                if stage_progress == previous_stage
        );

        assert!(runner.validate_execution(first_input, result.ok()).is_ok(), "validation failed");
    }

    struct SenderRecoveryTestRunner {
        tx: TestTransaction,
        threshold: u64,
    }

    impl Default for SenderRecoveryTestRunner {
        fn default() -> Self {
            Self { threshold: 1000, tx: TestTransaction::default() }
        }
    }

    impl SenderRecoveryTestRunner {
        fn set_threshold(&mut self, threshold: u64) {
            self.threshold = threshold;
        }

        /// # Panics
        ///
        /// 1. If there are any entries in the [tables::TxSenders] table above
        ///    a given block number.
        ///
        /// 2. If the is no requested block entry in the bodies table,
        ///    but [tables::TxSenders] is not empty.
        fn ensure_no_senders_by_block(&self, block: BlockNumber) -> Result<(), TestRunnerError> {
            let body_result = self.tx.inner().block_body_indices(block);
            match body_result {
                Ok(body) => self
                    .tx
                    .ensure_no_entry_above::<tables::TxSenders, _>(body.last_tx_num(), |key| key)?,
                Err(_) => {
                    assert!(self.tx.table_is_empty::<tables::TxSenders>()?);
                }
            };

            Ok(())
        }
    }

    impl StageTestRunner for SenderRecoveryTestRunner {
        type S = SenderRecoveryStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            SenderRecoveryStage { commit_threshold: self.threshold }
        }
    }

    impl ExecuteStageTestRunner for SenderRecoveryTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress();

            let blocks = random_block_range(stage_progress..=end, H256::zero(), 0..2);
            self.tx.insert_blocks(blocks.iter(), None)?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            match output {
                Some(output) => self.tx.query(|tx| {
                    let start_block = input.stage_progress.unwrap_or_default() + 1;
                    let end_block = output.stage_progress;

                    if start_block > end_block {
                        return Ok(())
                    }

                    let mut body_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
                    body_cursor.seek_exact(start_block)?;

                    while let Some((_, body)) = body_cursor.next()? {
                        for tx_id in body.tx_num_range() {
                            let transaction: TransactionSigned = tx
                                .get::<tables::Transactions>(tx_id)?
                                .expect("no transaction entry")
                                .into();
                            let signer =
                                transaction.recover_signer().expect("failed to recover signer");
                            assert_eq!(Some(signer), tx.get::<tables::TxSenders>(tx_id)?);
                        }
                    }

                    Ok(())
                })?,
                None => {
                    self.ensure_no_senders_by_block(input.stage_progress.unwrap_or_default())?
                }
            };

            Ok(())
        }
    }

    impl UnwindStageTestRunner for SenderRecoveryTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.ensure_no_senders_by_block(input.unwind_to)
        }
    }
}
