use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_interfaces::p2p::headers::client::StatusUpdater;
use reth_primitives::{BlockNumber, Head};
use reth_provider::Transaction;

/// The [`StageId`] of the finish stage.
pub const FINISH: StageId = StageId("Finish");

/// The finish stage.
///
/// This stage does not write anything; it's checkpoint is used to denote the highest fully synced
/// block, which is communicated to the P2P networking component, as well as the RPC component.
///
/// The highest block is communicated via a [StatusUpdater] when the stage executes or unwinds.
///
/// When the node starts up, the checkpoint for this stage should be used send the initial status
/// update to the relevant components. Assuming the genesis block is written before this, it is safe
/// to default the stage checkpoint to block number 0 on startup.
#[derive(Default, Debug, Clone)]
pub struct FinishStage<S> {
    updater: S,
}

impl<S> FinishStage<S>
where
    S: StatusUpdater,
{
    /// Create a new [FinishStage] with the given [StatusUpdater].
    pub fn new(updater: S) -> Self {
        Self { updater }
    }

    /// Construct a [Head] from `block_number`.
    fn fetch_head<DB: Database>(
        &self,
        tx: &mut Transaction<'_, DB>,
        block_number: BlockNumber,
    ) -> Result<Head, StageError> {
        let header = tx.get_header(block_number)?;
        let hash = tx.get_block_hash(block_number)?;
        let total_difficulty = tx.get_td(block_number)?;

        Ok(Head {
            number: block_number,
            hash,
            total_difficulty,
            difficulty: header.difficulty,
            timestamp: header.timestamp,
        })
    }
}

#[async_trait::async_trait]
impl<DB: Database, S> Stage<DB> for FinishStage<S>
where
    S: StatusUpdater,
{
    fn id(&self) -> StageId {
        FINISH
    }

    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        self.updater.update_status(self.fetch_head(tx, input.previous_stage_progress())?);
        Ok(ExecOutput { done: true, stage_progress: input.previous_stage_progress() })
    }

    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.updater.update_status(self.fetch_head(tx, input.unwind_to)?);
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner,
    };
    use reth_interfaces::test_utils::{
        generators::{random_header, random_header_range},
        TestStatusUpdater,
    };
    use reth_primitives::SealedHeader;
    use std::sync::Mutex;

    stage_test_suite_ext!(FinishTestRunner, finish);

    struct FinishTestRunner {
        tx: TestTransaction,
        status: Mutex<Option<tokio::sync::watch::Receiver<Head>>>,
    }

    impl FinishTestRunner {
        /// Gets the current status.
        ///
        /// # Panics
        ///
        /// Panics if multiple threads try to read the status at the same time, or if no status
        /// receiver has been set.
        fn current_status(&self) -> Head {
            let status_lock = self.status.try_lock().expect("competing for status lock");
            let status = status_lock.as_ref().expect("no status receiver set").borrow();
            status.clone()
        }
    }

    impl Default for FinishTestRunner {
        fn default() -> Self {
            FinishTestRunner { tx: TestTransaction::default(), status: Mutex::new(None) }
        }
    }

    impl StageTestRunner for FinishTestRunner {
        type S = FinishStage<TestStatusUpdater>;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            let (status_updater, status) = TestStatusUpdater::new();
            let mut status_container = self.status.try_lock().expect("competing for status lock");
            *status_container = Some(status);
            FinishStage::new(status_updater)
        }
    }

    impl ExecuteStageTestRunner for FinishTestRunner {
        type Seed = Vec<SealedHeader>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let start = input.stage_progress.unwrap_or_default();
            let head = random_header(start, None);
            self.tx.insert_headers_with_td(std::iter::once(&head))?;

            // use previous progress as seed size
            let end = input.previous_stage.map(|(_, num)| num).unwrap_or_default() + 1;

            if start + 1 >= end {
                return Ok(Vec::default())
            }

            let mut headers = random_header_range(start + 1..end, head.hash());
            self.tx.insert_headers_with_td(headers.iter())?;
            headers.insert(0, head);
            Ok(headers)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                assert!(output.done, "stage should always be done");
                assert_eq!(
                    output.stage_progress,
                    input.previous_stage_progress(),
                    "stage progress should always match progress of previous stage"
                );
                assert_eq!(
                    self.current_status().number,
                    output.stage_progress,
                    "incorrect block number in status update"
                );
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for FinishTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            assert_eq!(
                self.current_status().number,
                input.unwind_to,
                "incorrect block number in status update"
            );
            Ok(())
        }
    }
}
