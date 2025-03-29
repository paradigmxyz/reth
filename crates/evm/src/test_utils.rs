//! Helpers for testing.

use crate::{
    execute::{BasicBlockExecutor, BlockExecutionOutput, BlockExecutorProvider, Executor},
    Database, OnStateHook,
};
use alloc::{sync::Arc, vec::Vec};
use alloy_eips::eip7685::Requests;
use parking_lot::Mutex;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionResult, ExecutionOutcome};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock};
use revm::database::State;

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

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
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
            result: BlockExecutionResult {
                receipts: receipts.into_iter().flatten().collect(),
                requests: requests.into_iter().fold(Requests::default(), |mut reqs, req| {
                    reqs.extend(req);
                    reqs
                }),
                gas_used: 0,
            },
        })
    }

    fn execute_with_state_closure<F>(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _f: F,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: FnMut(&revm::database::State<DB>),
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

    fn into_state(self) -> revm::database::State<DB> {
        unreachable!()
    }

    fn size_hint(&self) -> usize {
        0
    }
}

impl<Factory, DB> BasicBlockExecutor<Factory, DB> {
    /// Provides safe read access to the state
    pub fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&State<DB>) -> R,
    {
        f(&self.db)
    }

    /// Provides safe write access to the state
    pub fn with_state_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut State<DB>) -> R,
    {
        f(&mut self.db)
    }
}
