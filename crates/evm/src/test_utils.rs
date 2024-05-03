//! Helpers for testing.

use crate::execute::{
    BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput,
    BlockExecutorProvider, Executor,
};
use parking_lot::Mutex;
use reth_interfaces::{executor::BlockExecutionError, provider::ProviderError};
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt};
use revm_primitives::db::Database;
use std::sync::Arc;

/// A [BlockExecutorProvider] that returns mocked execution results.
#[derive(Clone, Debug, Default)]
pub struct MockExecutorProvider {
    exec_results: Arc<Mutex<Vec<BatchBlockExecutionOutput>>>,
}

impl MockExecutorProvider {
    /// Extend the mocked execution results
    pub fn extend(&self, results: impl IntoIterator<Item = impl Into<BatchBlockExecutionOutput>>) {
        self.exec_results.lock().extend(results.into_iter().map(Into::into));
    }
}

impl BlockExecutorProvider for MockExecutorProvider {
    type Executor<DB: Database<Error = ProviderError>> = Self;

    type BatchExecutor<DB: Database<Error = ProviderError>> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        self.clone()
    }

    fn batch_executor<DB>(&self, _: DB, _: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        self.clone()
    }
}

impl<DB> Executor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BatchBlockExecutionOutput { bundle, receipts, .. } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            gas_used: 0,
        })
    }
}

impl<DB> BatchExecutor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BatchBlockExecutionOutput;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finalize(self) -> Self::Output {
        self.exec_results.lock().pop().unwrap()
    }

    fn set_tip(&mut self, _: BlockNumber) {}

    fn size_hint(&self) -> Option<usize> {
        None
    }
}
