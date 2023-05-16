use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_provider::Transaction;

/// The [`StageId`] of the finish stage.
pub const FINISH: StageId = StageId("Finish");

/// The finish stage.
///
/// This stage does not write anything; it's checkpoint is used to denote the highest fully synced
/// block.
#[derive(Default, Debug, Clone)]
pub struct FinishStage;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for FinishStage {
    fn id(&self) -> StageId {
        FINISH
    }

    async fn execute(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput { done: true, stage_progress: input.previous_stage_progress() })
    }

    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
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
    use reth_interfaces::test_utils::generators::{random_header, random_header_range};
    use reth_primitives::SealedHeader;

    stage_test_suite_ext!(FinishTestRunner, finish);

    struct FinishTestRunner {
        tx: TestTransaction,
    }

    impl Default for FinishTestRunner {
        fn default() -> Self {
            FinishTestRunner { tx: TestTransaction::default() }
        }
    }

    impl StageTestRunner for FinishTestRunner {
        type S = FinishStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            FinishStage
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
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for FinishTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            Ok(())
        }
    }
}
