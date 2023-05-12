use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{consensus::Consensus, provider::ProviderError};
use reth_primitives::U256;
use reth_provider::Transaction;
use std::sync::Arc;
use tracing::*;

/// The [`StageId`] of the total difficulty stage.
pub const TOTAL_DIFFICULTY: StageId = StageId("TotalDifficulty");

/// The total difficulty stage.
///
/// This stage walks over inserted headers and computes total difficulty
/// at each block. The entries are inserted into [`HeaderTD`][reth_db::tables::HeaderTD]
/// table.
#[derive(Debug, Clone)]
pub struct TotalDifficultyStage {
    /// Consensus client implementation
    consensus: Arc<dyn Consensus>,
    /// The number of table entries to commit at once
    commit_threshold: u64,
}

impl TotalDifficultyStage {
    /// Create a new total difficulty stage
    pub fn new(consensus: Arc<dyn Consensus>) -> Self {
        Self { consensus, commit_threshold: 100_000 }
    }

    /// Set a commit threshold on total difficulty stage
    pub fn with_commit_threshold(mut self, commit_threshold: u64) -> Self {
        self.commit_threshold = commit_threshold;
        self
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TotalDifficultyStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        TOTAL_DIFFICULTY
    }

    /// Write total difficulty entries
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);
        let (start_block, end_block) = range.clone().into_inner();

        debug!(target: "sync::stages::total_difficulty", start_block, end_block, "Commencing sync");

        // Acquire cursor over total difficulty and headers tables
        let mut cursor_td = tx.cursor_write::<tables::HeaderTD>()?;
        let mut cursor_headers = tx.cursor_read::<tables::Headers>()?;

        // Get latest total difficulty
        let last_header_number = input.stage_progress.unwrap_or_default();
        let last_entry = cursor_td
            .seek_exact(last_header_number)?
            .ok_or(ProviderError::TotalDifficulty { number: last_header_number })?;

        let mut td: U256 = last_entry.1.into();
        debug!(target: "sync::stages::total_difficulty", ?td, block_number = last_header_number, "Last total difficulty entry");

        // Walk over newly inserted headers, update & insert td
        for entry in cursor_headers.walk_range(range)? {
            let (block_number, header) = entry?;
            td += header.difficulty;

            self.consensus
                .validate_header_with_total_difficulty(&header, td)
                .map_err(|error| StageError::Validation { block: header.number, error })?;
            cursor_td.append(block_number, td.into())?;
        }
        info!(target: "sync::stages::total_difficulty", stage_progress = end_block, is_final_range, "Stage iteration finished");
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

        tx.unwind_table_by_num::<tables::HeaderTD>(unwind_to)?;

        info!(target: "sync::stages::total_difficulty", to_block = input.unwind_to, unwind_progress = unwind_to, is_final_range, "Unwind iteration finished");
        Ok(UnwindOutput { stage_progress: unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use reth_db::transaction::DbTx;
    use reth_interfaces::test_utils::{
        generators::{random_header, random_header_range},
        TestConsensus,
    };
    use reth_primitives::{BlockNumber, SealedHeader};

    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner, PREV_STAGE_ID,
    };

    stage_test_suite_ext!(TotalDifficultyTestRunner, total_difficulty);

    #[tokio::test]
    async fn execute_with_intermediate_commit() {
        let threshold = 50;
        let (stage_progress, previous_stage) = (1000, 1100); // input exceeds threshold

        let mut runner = TotalDifficultyTestRunner::default();
        runner.set_threshold(threshold);

        let first_input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        // Seed only once with full input range
        runner.seed_execution(first_input).expect("failed to seed execution");

        // Execute first time
        let result = runner.execute(first_input).await.unwrap();
        let expected_progress = stage_progress + threshold;
        assert!(matches!(
            result,
            Ok(ExecOutput { done: false, stage_progress })
                if stage_progress == expected_progress
        ));

        // Execute second time
        let second_input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(expected_progress),
        };
        let result = runner.execute(second_input).await.unwrap();
        assert!(matches!(
            result,
            Ok(ExecOutput { done: true, stage_progress })
                if stage_progress == previous_stage
        ));

        assert!(runner.validate_execution(first_input, result.ok()).is_ok(), "validation failed");
    }

    struct TotalDifficultyTestRunner {
        tx: TestTransaction,
        consensus: Arc<TestConsensus>,
        commit_threshold: u64,
    }

    impl Default for TotalDifficultyTestRunner {
        fn default() -> Self {
            Self {
                tx: Default::default(),
                consensus: Arc::new(TestConsensus::default()),
                commit_threshold: 500,
            }
        }
    }

    impl StageTestRunner for TotalDifficultyTestRunner {
        type S = TotalDifficultyStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            TotalDifficultyStage {
                consensus: self.consensus.clone(),
                commit_threshold: self.commit_threshold,
            }
        }
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for TotalDifficultyTestRunner {
        type Seed = Vec<SealedHeader>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let start = input.stage_progress.unwrap_or_default();
            let head = random_header(start, None);
            self.tx.insert_headers(std::iter::once(&head))?;
            self.tx.commit(|tx| {
                let td: U256 = tx
                    .cursor_read::<tables::HeaderTD>()?
                    .last()?
                    .map(|(_, v)| v)
                    .unwrap_or_default()
                    .into();
                tx.put::<tables::HeaderTD>(head.number, (td + head.difficulty).into())
            })?;

            // use previous progress as seed size
            let end = input.previous_stage.map(|(_, num)| num).unwrap_or_default() + 1;

            if start + 1 >= end {
                return Ok(Vec::default())
            }

            let mut headers = random_header_range(start + 1..end, head.hash());
            self.tx.insert_headers(headers.iter())?;
            headers.insert(0, head);
            Ok(headers)
        }

        /// Validate stored headers
        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            let initial_stage_progress = input.stage_progress.unwrap_or_default();
            match output {
                Some(output) if output.stage_progress > initial_stage_progress => {
                    self.tx.query(|tx| {
                        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;
                        let (_, mut current_header) = header_cursor
                            .seek_exact(initial_stage_progress)?
                            .expect("no initial header");
                        let mut td: U256 = tx
                            .get::<tables::HeaderTD>(initial_stage_progress)?
                            .expect("no initial td")
                            .into();

                        while let Some((next_key, next_header)) = header_cursor.next()? {
                            assert_eq!(current_header.number + 1, next_header.number);
                            td += next_header.difficulty;
                            assert_eq!(
                                tx.get::<tables::HeaderTD>(next_key)?.map(Into::into),
                                Some(td)
                            );
                            current_header = next_header;
                        }
                        Ok(())
                    })?;
                }
                _ => self.check_no_td_above(initial_stage_progress)?,
            };
            Ok(())
        }
    }

    impl UnwindStageTestRunner for TotalDifficultyTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.check_no_td_above(input.unwind_to)
        }
    }

    impl TotalDifficultyTestRunner {
        fn check_no_td_above(&self, block: BlockNumber) -> Result<(), TestRunnerError> {
            self.tx.ensure_no_entry_above::<tables::HeaderTD, _>(block, |num| num)?;
            Ok(())
        }

        fn set_threshold(&mut self, new_threshold: u64) {
            self.commit_threshold = new_threshold;
        }
    }
}
