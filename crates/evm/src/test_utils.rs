//! Helpers for testing.

use crate::{
    execute::{
        BatchExecutor, BlockExecutionInput, BlockExecutionOutput, BlockExecutorProvider, Executor,
    },
    system_calls::OnStateHook,
};
use alloy_primitives::BlockNumber;
use parking_lot::Mutex;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{BlockWithSenders, Receipt};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderError;
use revm::State;
use revm_primitives::db::Database;
use std::{fmt::Display, sync::Arc};

/// A [`BlockExecutorProvider`] that returns mocked execution results.
#[derive(Clone, Debug, Default)]
pub struct MockExecutorProvider {
    exec_results: Arc<Mutex<Vec<ExecutionOutcome>>>,
}

impl MockExecutorProvider {
    /// Extend the mocked execution results
    pub fn extend(&self, results: impl IntoIterator<Item = impl Into<ExecutionOutcome>>) {
        self.exec_results.lock().extend(results.into_iter().map(Into::into));
    }
}

impl BlockExecutorProvider for MockExecutorProvider {
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.clone()
    }

    fn batch_executor<DB>(&self, _: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.clone()
    }
}

impl<DB> Executor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            requests: requests.into_iter().flatten().collect(),
            gas_used: 0,
        })
    }

    fn execute_with_state_closure<F>(
        self,
        input: Self::Input<'_>,
        _: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        <Self as Executor<DB>>::execute(self, input)
    }

    fn execute_with_state_hook<F>(
        self,
        input: Self::Input<'_>,
        _: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook,
    {
        <Self as Executor<DB>>::execute(self, input)
    }
}

impl<DB> BatchExecutor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finalize(self) -> Self::Output {
        self.exec_results.lock().pop().unwrap()
    }

    fn set_tip(&mut self, _: BlockNumber) {}

    fn set_prune_modes(&mut self, _: PruneModes) {}

    fn size_hint(&self) -> Option<usize> {
        None
    }
}
