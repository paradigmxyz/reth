use super::{TestStage, TEST_STAGE_ID};
use crate::{ExecOutput, StageError, StageSet, StageSetBuilder, UnwindOutput};
use reth_db::database::Database;
use std::collections::VecDeque;

#[derive(Default, Debug)]
pub struct TestStages {
    exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
}

impl TestStages {
    pub fn new(
        exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
    ) -> Self {
        Self { exec_outputs, unwind_outputs }
    }
}

impl<DB: Database> StageSet<DB> for TestStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default().add_stage(
            TestStage::new(TEST_STAGE_ID)
                .with_exec(self.exec_outputs)
                .with_unwind(self.unwind_outputs),
        )
    }
}
