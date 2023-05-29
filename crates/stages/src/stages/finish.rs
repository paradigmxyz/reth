use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::{stage::StageId, StageCheckpoint};
use reth_provider::Transaction;

/// The finish stage.
///
/// This stage does not write anything; it's checkpoint is used to denote the highest fully synced
/// block.
#[derive(Default, Debug, Clone)]
pub struct FinishStage;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for FinishStage {
    fn id(&self) -> StageId {
        StageId::Finish
    }

    async fn execute(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput { checkpoint: input.previous_stage_checkpoint(), done: true })
    }

    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
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

    #[derive(Default)]
    struct FinishTestRunner {
        tx: TestTransaction,
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
            let start = input.checkpoint().block_number;
            let head = random_header(start, None);
            self.tx.insert_headers_with_td(std::iter::once(&head))?;

            // use previous progress as seed size
            let end = input.previous_stage.map(|(_, num)| num).unwrap_or_default().block_number + 1;

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
                    output.checkpoint,
                    input.previous_stage_checkpoint(),
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
