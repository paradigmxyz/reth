//! Helpers for testing.

use crate::{
    execute::{
        BasicBatchExecutor, BasicBlockExecutor, BatchExecutor, BlockExecutionInput,
        BlockExecutionOutput, BlockExecutionStrategy, BlockExecutorProvider, Executor,
    },
    system_calls::OnStateHook,
};
use alloy_eips::eip7685::Requests;
use alloy_primitives::BlockNumber;
use parking_lot::Mutex;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::ExecutionOutcome;
use reth_node_types::NodePrimitives;
use reth_primitives::{BlockWithSenders, Receipts};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderError;
use revm::State;
use revm_primitives::db::Database;
use std::{fmt::Display, sync::Arc};

/// A [`BlockExecutorProvider`] that returns mocked execution results.
#[derive(Clone, Debug, Default)]
pub struct MockExecutorProvider<T = reth_primitives::Receipt> {
    exec_results: Arc<Mutex<Vec<ExecutionOutcome<T>>>>,
}

impl<T> MockExecutorProvider<T> {
    /// Extend the mocked execution results
    pub fn extend(&self, results: impl IntoIterator<Item = impl Into<ExecutionOutcome<T>>>) {
        self.exec_results.lock().extend(results.into_iter().map(Into::into));
    }
}

impl<N: NodePrimitives + 'static> BlockExecutorProvider<N> for MockExecutorProvider<N::Receipt> {
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

impl<DB, N> Executor<DB, N> for MockExecutorProvider<N::Receipt>
where
    N: NodePrimitives,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<N::Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                reqs.extend(req);
                reqs
            }),
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
        <Self as Executor<DB, N>>::execute(self, input)
    }

    fn execute_with_state_hook<F>(
        self,
        input: Self::Input<'_>,
        _: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook,
    {
        <Self as Executor<DB, N>>::execute(self, input)
    }
}

impl<DB, N> BatchExecutor<DB, N> for MockExecutorProvider<N::Receipt>
where
    N: NodePrimitives,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome<N::Receipt>;
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

impl<S, DB, N> BasicBlockExecutor<S, DB, N>
where
    S: BlockExecutionStrategy<DB, N>,
    DB: Database,
    N: NodePrimitives,
{
    /// Provides safe read access to the state
    pub fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&State<DB>) -> R,
    {
        f(self.strategy.state_ref())
    }

    /// Provides safe write access to the state
    pub fn with_state_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut State<DB>) -> R,
    {
        f(self.strategy.state_mut())
    }
}

impl<S, DB, N> BasicBatchExecutor<S, DB, N>
where
    S: BlockExecutionStrategy<DB, N>,
    DB: Database,
    N: NodePrimitives,
{
    /// Provides safe read access to the state
    pub fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&State<DB>) -> R,
    {
        f(self.strategy.state_ref())
    }

    /// Provides safe write access to the state
    pub fn with_state_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut State<DB>) -> R,
    {
        f(self.strategy.state_mut())
    }

    /// Accessor for batch executor receipts.
    pub const fn receipts(&self) -> &Receipts<N::Receipt> {
        self.batch_record.receipts()
    }
}
