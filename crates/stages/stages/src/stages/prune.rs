use reth_db_api::database::Database;
use reth_provider::{DatabaseProviderRW, PruneCheckpointReader};
use reth_prune::{PruneModes, PrunerBuilder};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use tracing::info;

#[derive(Debug)]
pub struct PruneStage {
    prune_modes: PruneModes,
    commit_threshold: usize,
}

impl PruneStage {
    pub const fn new(prune_modes: PruneModes, commit_threshold: usize) -> Self {
        Self { prune_modes, commit_threshold }
    }
}

impl<DB: Database> Stage<DB> for PruneStage {
    fn id(&self) -> StageId {
        StageId::Prune
    }

    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let mut pruner = PrunerBuilder::default()
            .segments(self.prune_modes.clone())
            .prune_max_blocks_per_run(1)
            .delete_limit_per_block(self.commit_threshold)
            .build();

        let result = pruner.run(provider, input.target())?;
        let lowest_pruned_block = provider
            .get_prune_checkpoints()?
            .into_iter()
            .map(|(_, checkpoint)| checkpoint.block_number)
            .min()
            .flatten()
            .unwrap_or_else(|| input.target());

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(lowest_pruned_block),
            done: result.is_finished(),
        })
    }

    fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::prune::unwind", "Stage is always skipped");
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
