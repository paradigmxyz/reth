//! Helpers for testing.

use crate::{
    execute::{
        BasicBatchExecutor, BasicBlockExecutor, BatchExecutor, BlockExecutionOutput,
        BlockExecutionStrategy, BlockExecutorProvider, Executor,
    },
    system_calls::OnStateHook,
    Database,
};
use alloy_eips::eip7685::Requests;
use parking_lot::Mutex;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionResult, ExecutionOutcome};
use reth_primitives::{EthPrimitives, NodePrimitives, RecoveredBlock};
use revm::State;
use std::sync::Arc;

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
    type Primitives = EthPrimitives;

    type Executor<DB: Database> = Self;

    type BatchExecutor<DB: Database> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database,
    {
        self.clone()
    }

    fn batch_executor<DB>(&self, _: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database,
    {
        self.clone()
    }
}

impl<DB: Database> Executor<DB> for MockExecutorProvider {
    type Primitives = EthPrimitives;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let ExecutionOutcome { bundle: _, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionResult {
            receipts: receipts.into_iter().flatten().collect(),
            requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                reqs.extend(req);
                reqs
            }),
            gas_used: 0,
        })
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        <Self as Executor<DB>>::execute_one(self, block)
    }

    fn execute(
        self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().collect(),
            requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                reqs.extend(req);
                reqs
            }),
            gas_used: 0,
        })
    }

    fn execute_with_state_closure<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&revm::db::State<DB>),
    {
        <Self as Executor<DB>>::execute(self, block)
    }

    fn execute_with_state_hook<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        <Self as Executor<DB>>::execute(self, block)
    }

    fn into_state(self) -> revm::db::State<DB> {
        unreachable!()
    }

    fn size_hint(&self) -> usize {
        0
    }
}

impl<DB> BatchExecutor<DB> for MockExecutorProvider {
    type Input<'a> = &'a RecoveredBlock<reth_primitives::Block>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finalize(self) -> Self::Output {
        self.exec_results.lock().pop().unwrap()
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

impl<S> BasicBlockExecutor<S>
where
    S: BlockExecutionStrategy,
{
    /// Provides safe read access to the state
    pub fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&State<S::DB>) -> R,
    {
        f(self.strategy.state_ref())
    }

    /// Provides safe write access to the state
    pub fn with_state_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut State<S::DB>) -> R,
    {
        f(self.strategy.state_mut())
    }
}

impl<S> BasicBatchExecutor<S>
where
    S: BlockExecutionStrategy,
{
    /// Provides safe read access to the state
    pub fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&State<S::DB>) -> R,
    {
        f(self.strategy.state_ref())
    }

    /// Provides safe write access to the state
    pub fn with_state_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut State<S::DB>) -> R,
    {
        f(self.strategy.state_mut())
    }

    /// Accessor for batch executor receipts.
    pub const fn receipts(&self) -> &Vec<Vec<<S::Primitives as NodePrimitives>::Receipt>> {
        self.batch_record.receipts()
    }
}
