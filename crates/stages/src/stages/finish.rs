use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::database::Database;

/// The [`StageId`] of the finish stage.
pub const FINISH: StageId = StageId("Finish");

/// The finish stage.
///
/// This stage does nothing; it's checkpoint is used to denote the highest fully synced block,
/// which is communicated to the P2P networking component, as well as the RPC component.
#[derive(Default, Debug, Clone)]
pub struct FinishStage;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for FinishStage {
    fn id(&self) -> StageId {
        FINISH
    }

    async fn execute(
        &mut self,
        _: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput { done: true, stage_progress: input.previous_stage_progress() })
    }

    async fn unwind(
        &mut self,
        _: &mut Transaction<'_, DB>,
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

    stage_test_suite_ext!(FinishTestRunner, finish);

    struct FinishTestRunner {
        tx: TestTransaction,
    }

    impl Default for FinishTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default() }
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
        type Seed = ();

        fn seed_execution(&mut self, _: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            Ok(())
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                assert!(output.done);
                assert_eq!(output.stage_progress, input.previous_stage_progress());
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for FinishTestRunner {
        fn validate_unwind(&self, _: UnwindInput) -> Result<(), TestRunnerError> {
            Ok(())
        }
    }
}
