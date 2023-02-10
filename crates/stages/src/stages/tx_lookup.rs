use crate::{
    db::Transaction, exec_or_return, ExecAction, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use tracing::*;

const TRANSACTION_LOOKUP: StageId = StageId("TransactionLookup");

/// The transaction lookup stage.
///
/// This stage walks over the bodies table, and sets the transaction hash of each transaction in a
/// block to the corresponding `TransitionId` at each block. This is written to the
/// [`tables::TxHashNumber`] This is used for looking up changesets via the transaction hash.
#[derive(Debug, Clone)]
pub struct TransactionLookupStage {
    /// The number of table entries to commit at once
    commit_threshold: u64,
}

impl Default for TransactionLookupStage {
    fn default() -> Self {
        Self { commit_threshold: 50_000 }
    }
}

impl TransactionLookupStage {
    /// Create new instance of [TransactionLookupStage].
    pub fn new(commit_threshold: u64) -> Self {
        Self { commit_threshold }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TransactionLookupStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        TRANSACTION_LOOKUP
    }

    /// Write total difficulty entries
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let ((start_block, end_block), capped) =
            exec_or_return!(input, self.commit_threshold, "sync::stages::transaction_lookup");

        debug!(target: "sync::stages::transaction_lookup", start_block, end_block, "Commencing sync");

        let mut cursor_bodies = tx.cursor_read::<tables::BlockBodies>()?;
        let mut tx_cursor = tx.cursor_write::<tables::Transactions>()?;

        // Walk over block bodies within a specified range.
        let bodies = cursor_bodies.walk(start_block)?.take_while(|entry| {
            entry.as_ref().map(|(num, _)| *num <= end_block).unwrap_or_default()
        });

        // Collect transactions for each body
        let mut tx_list = vec![];
        for body_entry in bodies {
            let (_, body) = body_entry?;
            let transactions = tx_cursor.walk(body.start_tx_id)?.take(body.tx_count as usize);

            for tx_entry in transactions {
                let (id, transaction) = tx_entry?;
                tx_list.push((transaction.hash(), id));
            }
        }

        // Sort before inserting the reverse lookup for hash -> tx_id.
        tx_list.sort_by(|txa, txb| txa.0.cmp(&txb.0));

        let mut txhash_cursor = tx.cursor_write::<tables::TxHashNumber>()?;

        // If the last inserted element in the database is smaller than the first in our set, then
        // we can just append into the DB. This probably only ever happens during sync, on
        // the first table insertion.
        let append = tx_list
            .first()
            .zip(txhash_cursor.last()?)
            .map(|((first, _), (last, _))| &last < first)
            .unwrap_or_default();

        for (tx_hash, id) in tx_list {
            if append {
                txhash_cursor.append(tx_hash, id)?;
            } else {
                txhash_cursor.insert(tx_hash, id)?;
            }
        }

        let done = !capped;
        info!(target: "sync::stages::transaction_lookup", stage_progress = end_block, done, "Sync iteration finished");
        Ok(ExecOutput { done, stage_progress: end_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::transaction_lookup", to_block = input.unwind_to, "Unwinding");
        // Cursors to unwind tx hash to number
        let mut body_cursor = tx.cursor_write::<tables::BlockBodies>()?;
        let mut tx_hash_number_cursor = tx.cursor_write::<tables::TxHashNumber>()?;
        let mut transaction_cursor = tx.cursor_write::<tables::Transactions>()?;
        let mut rev_walker = body_cursor.walk_back(None)?;
        while let Some((number, body)) = rev_walker.next().transpose()? {
            if number <= input.unwind_to {
                break
            }

            // Delete all transactions that belong to this block
            for tx_id in body.tx_id_range() {
                // First delete the transaction and hash to id mapping
                if let Some((_, transaction)) = transaction_cursor.seek_exact(tx_id)? {
                    if tx_hash_number_cursor.seek_exact(transaction.hash)?.is_some() {
                        tx_hash_number_cursor.delete_current()?;
                    }
                }
            }
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner, PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::{random_block, random_block_range};
    use reth_primitives::{BlockNumber, SealedBlock, H256};

    // Implement stage test suite.
    stage_test_suite_ext!(TransactionLookupTestRunner, transaction_lookup);

    #[tokio::test]
    async fn execute_single_transaction_lookup() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let runner = TransactionLookupTestRunner::default();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        // Insert blocks with a single transaction at block `stage_progress + 10`
        let non_empty_block_number = stage_progress + 10;
        let blocks = (stage_progress..input.previous_stage_progress() + 1)
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
    async fn execute_intermediate_commit_transaction_lookup() {
        let threshold = 50;
        let mut runner = TransactionLookupTestRunner::default();
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

    struct TransactionLookupTestRunner {
        tx: TestTransaction,
        threshold: u64,
    }

    impl Default for TransactionLookupTestRunner {
        fn default() -> Self {
            Self { threshold: 1000, tx: TestTransaction::default() }
        }
    }

    impl TransactionLookupTestRunner {
        fn set_threshold(&mut self, threshold: u64) {
            self.threshold = threshold;
        }

        /// # Panics
        ///
        /// 1. If there are any entries in the [tables::TxHashNumber] table above
        ///    a given block number.
        ///
        /// 2. If the is no requested block entry in the bodies table,
        ///    but [tables::TxHashNumber] is not empty.
        fn ensure_no_hash_by_block(&self, number: BlockNumber) -> Result<(), TestRunnerError> {
            let body_result = self.tx.inner().get_block_body(number);
            match body_result {
                Ok(body) => self.tx.ensure_no_entry_above_by_value::<tables::TxHashNumber, _>(
                    body.last_tx_index(),
                    |key| key,
                )?,
                Err(_) => {
                    assert!(self.tx.table_is_empty::<tables::TxHashNumber>()?);
                }
            };

            Ok(())
        }
    }

    impl StageTestRunner for TransactionLookupTestRunner {
        type S = TransactionLookupStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            TransactionLookupStage { commit_threshold: self.threshold }
        }
    }

    impl ExecuteStageTestRunner for TransactionLookupTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress() + 1;

            let blocks = random_block_range(stage_progress..end, H256::zero(), 0..2);
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

                    let mut body_cursor = tx.cursor_read::<tables::BlockBodies>()?;
                    body_cursor.seek_exact(start_block)?;

                    while let Some((_, body)) = body_cursor.next()? {
                        for tx_id in body.tx_id_range() {
                            let transaction = tx
                                .get::<tables::Transactions>(tx_id)?
                                .expect("no transaction entry");
                            assert_eq!(
                                Some(tx_id),
                                tx.get::<tables::TxHashNumber>(transaction.hash)?,
                            );
                        }
                    }

                    Ok(())
                })?,
                None => self.ensure_no_hash_by_block(input.stage_progress.unwrap_or_default())?,
            };
            Ok(())
        }
    }

    impl UnwindStageTestRunner for TransactionLookupTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.ensure_no_hash_by_block(input.unwind_to)
        }
    }
}
