#![allow(missing_docs)]

use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_errors::BlockExecError;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

/// A test stage that can be used for testing.
///
/// This can be used to mock expected outputs of [`Stage::execute`] and [`Stage::unwind`]
#[derive(Debug)]
pub struct TestStage<E: BlockExecError> {
    id: StageId,
    exec_outputs: VecDeque<Result<ExecOutput, StageError<E>>>,
    unwind_outputs: VecDeque<Result<UnwindOutput, StageError<E>>>,
    post_execute_commit_counter: Arc<AtomicUsize>,
    post_unwind_commit_counter: Arc<AtomicUsize>,
}

impl<E> TestStage<E>
where
    E: BlockExecError,
{
    pub fn new(id: StageId) -> Self {
        Self {
            id,
            exec_outputs: VecDeque::new(),
            unwind_outputs: VecDeque::new(),
            post_execute_commit_counter: Arc::new(AtomicUsize::new(0)),
            post_unwind_commit_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn with_exec(mut self, exec_outputs: VecDeque<Result<ExecOutput, StageError<E>>>) -> Self {
        self.exec_outputs = exec_outputs;
        self
    }

    pub fn with_unwind(
        mut self,
        unwind_outputs: VecDeque<Result<UnwindOutput, StageError<E>>>,
    ) -> Self {
        self.unwind_outputs = unwind_outputs;
        self
    }

    pub fn add_exec(mut self, output: Result<ExecOutput, StageError<E>>) -> Self {
        self.exec_outputs.push_back(output);
        self
    }

    pub fn add_unwind(mut self, output: Result<UnwindOutput, StageError<E>>) -> Self {
        self.unwind_outputs.push_back(output);
        self
    }

    pub fn with_post_execute_commit_counter(mut self) -> (Self, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        self.post_execute_commit_counter = counter.clone();
        (self, counter)
    }

    pub fn with_post_unwind_commit_counter(mut self) -> (Self, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        self.post_unwind_commit_counter = counter.clone();
        (self, counter)
    }
}

impl<Provider, E> Stage<Provider, E> for TestStage<E>
where
    E: BlockExecError,
{
    fn id(&self) -> StageId {
        self.id
    }

    fn execute(&mut self, _: &Provider, _input: ExecInput) -> Result<ExecOutput, StageError<E>> {
        self.exec_outputs
            .pop_front()
            .unwrap_or_else(|| panic!("Test stage {} executed too many times.", self.id))
    }

    fn post_execute_commit(&mut self) -> Result<(), StageError<E>> {
        self.post_execute_commit_counter.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    fn unwind(&mut self, _: &Provider, _input: UnwindInput) -> Result<UnwindOutput, StageError<E>> {
        self.unwind_outputs
            .pop_front()
            .unwrap_or_else(|| panic!("Test stage {} unwound too many times.", self.id))
    }

    fn post_unwind_commit(&mut self) -> Result<(), StageError<E>> {
        self.post_unwind_commit_counter.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }
}
