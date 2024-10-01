use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};

/// The finish stage.
///
/// This stage does not write anything; it's checkpoint is used to denote the highest fully synced
/// block.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct FinishStage;

impl<Provider> Stage<Provider> for FinishStage {
    fn id(&self) -> StageId {
        StageId::Finish
    }

    fn execute(
        &mut self,
        _provider: &Provider,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true })
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
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
        TestStageDB, UnwindStageTestRunner,
    };
    use reth_primitives::SealedHeader;
    use reth_provider::providers::StaticFileWriter;
    use reth_testing_utils::{
        generators,
        generators::{random_header, random_header_range},
    };

    stage_test_suite_ext!(FinishTestRunner, finish);

    #[derive(Default)]
    struct FinishTestRunner {
        db: TestStageDB,
    }

    impl StageTestRunner for FinishTestRunner {
        type S = FinishStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            FinishStage
        }
    }

    impl ExecuteStageTestRunner for FinishTestRunner {
        type Seed = Vec<SealedHeader>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let start = input.checkpoint().block_number;
            let mut rng = generators::rng();
            let head = random_header(&mut rng, start, None);
            self.db.insert_headers_with_td(std::iter::once(&head))?;

            // use previous progress as seed size
            let end = input.target.unwrap_or_default() + 1;

            if start + 1 >= end {
                return Ok(Vec::default())
            }

            let mut headers = random_header_range(&mut rng, start + 1..end, head.hash());
            self.db.insert_headers_with_td(headers.iter())?;
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
                    output.checkpoint.block_number,
                    input.target(),
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
