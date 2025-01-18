use super::TEST_STAGE_ID;
use crate::{StageSet, StageSetBuilder};
use reth_execution_errors::GenericBlockExecutionError;
use reth_stages_api::{test_utils::TestStage, ExecOutput, StageError, UnwindOutput};
use std::collections::VecDeque;

#[derive(Default, Debug)]
pub struct TestStages<E>
where
    E: GenericBlockExecutionError,
{
    exec_outputs: VecDeque<Result<ExecOutput, StageError<E>>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError<E>>>,
}

impl<E> TestStages<E>
where
    E: GenericBlockExecutionError,
{
    pub const fn new(
        exec_outputs: VecDeque<Result<ExecOutput, StageError<E>>>,
        unwind_outputs: VecDeque<Result<UnwindOutput, StageError<E>>>,
    ) -> Self {
        Self { exec_outputs, unwind_outputs }
    }
}

impl<Provider, E> StageSet<Provider, E> for TestStages<E>
where
    E: GenericBlockExecutionError,
{
    fn builder(self) -> StageSetBuilder<Provider, E> {
        StageSetBuilder::default().add_stage(
            TestStage::new(TEST_STAGE_ID)
                .with_exec(self.exec_outputs)
                .with_unwind(self.unwind_outputs),
        )
    }
}
