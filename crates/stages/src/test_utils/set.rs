use super::{TestStage, TEST_STAGE_ID};
use crate::{ExecOutput, StageError, StageSet, StageSetBuilder, UnwindOutput};
use reth_db::database::Database;
use std::{collections::VecDeque, time::Duration};

#[derive(Default, Debug)]
pub struct TestStages {
    exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
    delay: Option<Duration>,
}

impl TestStages {
    pub fn with_exec_outputs(
        mut self,
        exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    ) -> Self {
        self.exec_outputs = exec_outputs;
        self
    }

    pub fn with_unwind_outputs(
        mut self,
        unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
    ) -> Self {
        self.unwind_outputs = unwind_outputs;
        self
    }

    pub fn with_delay(mut self, duration: Option<Duration>) -> Self {
        self.delay = duration;
        self
    }
}

impl<DB: Database> StageSet<DB> for TestStages {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default().add_stage(
            TestStage::new(TEST_STAGE_ID)
                .with_exec(self.exec_outputs)
                .with_unwind(self.unwind_outputs)
                .with_delay(self.delay),
        )
    }
}
