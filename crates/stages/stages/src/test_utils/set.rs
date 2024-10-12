use super::TEST_STAGE_ID;
use crate::{StageSet, StageSetBuilder};
use reth_db_api::database::Database;
use reth_stages_api::{test_utils::TestStage, ExecOutput, StageError, UnwindOutput};
use std::collections::VecDeque;

#[derive(Default, Debug)]
pub struct TestStages {
    exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
}

impl TestStages {
    pub const fn new(
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
