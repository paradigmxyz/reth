use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::stage::{StageCheckpoint, StageId};
use reth_provider::{DatabaseProviderRW, LogHistoryWriter};

/// The log indexing stage.
///
/// This stage walks over all available receipts and inserts the index of block numbers where
/// particular log addresses and topics occurred.
///
/// For more information see [reth_db::tables::LogAddressHistory] and
/// [reth_db::tables::LogTopicHistory].
#[derive(Debug, Clone)]
pub struct IndexLogHistoryStage {
    commit_threshold: u64,
}

impl Default for IndexLogHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for IndexLogHistoryStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexLogHistory
    }

    /// Execute the stage.
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        // TODO: make tx block range
        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);

        let (log_address_indices, log_topic_indices, _) =
            provider.compute_log_indexes(range.clone())?;
        provider.insert_log_address_history_index(log_address_indices)?;
        provider.insert_log_topic_history_index(log_topic_indices)?;

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(*range.end()), done: is_final_range })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        provider.unwind_log_history_indexes(range)?;

        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_progress) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestStageDB, UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_db::{
        cursor::DbCursorRO,
        models::{log_sharded_key::LogShardedKey, ShardedKey},
        tables,
        transaction::DbTx,
    };
    use reth_interfaces::test_utils::generators::{random_block_range, random_log};
    use reth_primitives::{Address, BlockNumber, Receipt, SealedBlock, B256};

    stage_test_suite_ext!(IndexLogHistoryTestRunner, index_log_history);

    /// Execute the stage twice with input range that exceeds the commit threshold
    #[tokio::test]
    async fn execute_intermediate_commit() {
        let threshold = 50;
        let mut runner = IndexLogHistoryTestRunner::default();
        runner.set_threshold(threshold);
        let (stage_progress, target) = (1000, 1100); // input exceeds threshold

        let first_input = ExecInput {
            target: Some(target),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };
        let (_, seed_receipts) =
            runner.seed_execution(first_input).expect("failed to seed execution");

        let total_receipts = runner.db.table::<tables::Receipts>().unwrap().len() as u64;
        assert_eq!(seed_receipts.len() as u64, total_receipts);

        // Execute first time
        let result = runner.execute(first_input).await.unwrap();
        let expected_progress = stage_progress + threshold;
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.unwrap(),
            ExecOutput { checkpoint: StageCheckpoint::new(expected_progress), done: false }
        );

        // Execute second time to completion
        let second_input = ExecInput {
            target: Some(target),
            checkpoint: Some(StageCheckpoint::new(expected_progress)),
        };
        let result = runner.execute(second_input).await.unwrap();
        assert_matches!(result, Ok(_));
        assert_eq!(
            result.as_ref().unwrap(),
            &ExecOutput { checkpoint: StageCheckpoint::new(target), done: true }
        );

        assert!(runner.validate_execution(first_input, result.ok()).is_ok(), "validation failed");
    }

    struct IndexLogHistoryTestRunner {
        db: TestStageDB,
        threshold: u64,
    }

    impl Default for IndexLogHistoryTestRunner {
        fn default() -> Self {
            Self { threshold: 1000, db: TestStageDB::default() }
        }
    }

    impl IndexLogHistoryTestRunner {
        fn set_threshold(&mut self, threshold: u64) {
            self.threshold = threshold;
        }

        fn ensure_no_log_indices_by_block(
            &self,
            block: BlockNumber,
        ) -> Result<(), TestRunnerError> {
            self.db.query(|tx| {
                for entry in tx.cursor_read::<tables::LogAddressHistory>()?.walk_range(..)? {
                    let (_, block_indices) = entry?;
                    for block_number in block_indices.iter(0) {
                        assert!(block_number as u64 <= block);
                    }
                }
                for entry in tx.cursor_read::<tables::LogTopicHistory>()?.walk_range(..)? {
                    let (_, block_indices) = entry?;
                    for block_number in block_indices.iter(0) {
                        assert!(block_number as u64 <= block);
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
    }

    impl StageTestRunner for IndexLogHistoryTestRunner {
        type S = IndexLogHistoryStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            IndexLogHistoryStage { commit_threshold: self.threshold }
        }
    }

    impl ExecuteStageTestRunner for IndexLogHistoryTestRunner {
        type Seed = (Vec<SealedBlock>, Vec<Receipt>);

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let mut rng = rand::thread_rng();
            let stage_progress = input.checkpoint().block_number;
            let end = input.target();

            let tx_offset = None;

            let blocks = random_block_range(&mut rng, stage_progress + 1..=end, B256::ZERO, 1..4);
            self.db.insert_blocks(blocks.iter(), tx_offset)?;

            let recurring_address = Address::random();
            let logs_per_receipt = 3;
            let mut receipts = Vec::new();
            for block in &blocks {
                receipts.extend(block.body.iter().map(|tx| {
                    Receipt {
                        tx_type: tx.tx_type(),
                        success: true,
                        cumulative_gas_used: 100,
                        logs: (0..logs_per_receipt)
                            .map(|_| {
                                let address = rng.gen::<bool>().then_some(recurring_address);
                                random_log(&mut rng, address, None)
                            })
                            .collect(),
                    }
                }));
            }
            self.db.insert_receipts(receipts.iter().enumerate().map(|(idx, receipt)| {
                (idx as u64 + tx_offset.unwrap_or_default(), receipt.clone())
            }))?;

            Ok((blocks, receipts))
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            match output {
                Some(output) => self.db.query(|tx| {
                    let start_block = input.next_block();
                    let end_block = output.checkpoint.block_number;

                    if start_block > end_block {
                        return Ok(())
                    }

                    let mut body_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
                    body_cursor.seek_exact(start_block)?;

                    while let Some((block_number, body)) = body_cursor.next()? {
                        for tx_id in body.tx_num_range() {
                            let receipt =
                                tx.get::<tables::Receipts>(tx_id)?.expect("no receipt entry");
                            for log in &receipt.logs {
                                for topic in &log.topics {
                                    // Validate address index is present for this log address
                                    let address_index_entry =
                                        tx.cursor_read::<tables::LogAddressHistory>()?.seek(
                                            LogShardedKey::new(log.address, *topic, block_number),
                                        )?;
                                    assert_matches!(address_index_entry, Some(_));
                                    assert_matches!(
                                        address_index_entry.unwrap().1.find(block_number as usize),
                                        Some(_)
                                    );

                                    // Validate topic index is present for this log topic
                                    let topic_index_entry = tx
                                        .cursor_read::<tables::LogTopicHistory>()?
                                        .seek(ShardedKey::new(*topic, block_number))?;
                                    assert_matches!(topic_index_entry, Some(_));
                                    assert_matches!(
                                        topic_index_entry.unwrap().1.find(block_number as usize),
                                        Some(_)
                                    );
                                }
                            }
                        }
                    }

                    Ok(())
                })?,
                None => self.ensure_no_log_indices_by_block(input.checkpoint().block_number)?,
            };

            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexLogHistoryTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.ensure_no_log_indices_by_block(input.unwind_to)
        }
    }
}
