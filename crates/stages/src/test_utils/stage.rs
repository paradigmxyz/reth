use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::stage::StageId;
use reth_provider::DatabaseProviderRW;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct TestStage {
    id: StageId,
    exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
}

impl TestStage {
    pub fn new(id: StageId) -> Self {
        Self { id, exec_outputs: VecDeque::new(), unwind_outputs: VecDeque::new() }
    }

    pub fn with_exec(mut self, exec_outputs: VecDeque<Result<ExecOutput, StageError>>) -> Self {
        self.exec_outputs = exec_outputs;
        self
    }

    pub fn with_unwind(
        mut self,
        unwind_outputs: VecDeque<Result<UnwindOutput, StageError>>,
    ) -> Self {
        self.unwind_outputs = unwind_outputs;
        self
    }

    pub fn add_exec(mut self, output: Result<ExecOutput, StageError>) -> Self {
        self.exec_outputs.push_back(output);
        self
    }

    pub fn add_unwind(mut self, output: Result<UnwindOutput, StageError>) -> Self {
        self.unwind_outputs.push_back(output);
        self
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for TestStage {
    fn id(&self) -> StageId {
        self.id
    }

    async fn execute(
        &mut self,
        _: &DatabaseProviderRW<'_, &DB>,
        _input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        self.exec_outputs
            .pop_front()
            .unwrap_or_else(|| panic!("Test stage {} executed too many times.", self.id))
    }

    async fn unwind(
        &mut self,
        _: &DatabaseProviderRW<'_, &DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.unwind_outputs
            .pop_front()
            .unwrap_or_else(|| panic!("Test stage {} unwound too many times.", self.id))
    }
}
