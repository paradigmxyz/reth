//! A no operation block executor implementation.

use alloy_primitives::BlockNumber;
use core::fmt::Display;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionInput, BlockExecutionOutput, ExecutionOutcome};
use reth_primitives::{BlockWithSenders, Receipt};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderError;
use revm::State;
use revm_primitives::db::Database;

use crate::{
    execute::{BatchExecutor, BlockExecutorProvider, Executor},
    system_calls::OnStateHook,
};

const UNAVAILABLE_FOR_NOOP: &str = "execution unavailable for noop";

/// A [`BlockExecutorProvider`] implementation that does nothing.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct NoopBlockExecutorProvider;

impl BlockExecutorProvider for NoopBlockExecutorProvider {
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        Self
    }

    fn batch_executor<DB>(&self, _: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        Self
    }
}

impl<DB> Executor<DB> for NoopBlockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn execute_with_state_witness<F>(
        self,
        _: Self::Input<'_>,
        _: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn execute_with_state_hook<F>(
        self,
        _: Self::Input<'_>,
        _: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook,
    {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }
}

impl<DB> BatchExecutor<DB> for NoopBlockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn finalize(self) -> Self::Output {
        unreachable!()
    }

    fn set_tip(&mut self, _: BlockNumber) {}

    fn set_prune_modes(&mut self, _: PruneModes) {}

    fn size_hint(&self) -> Option<usize> {
        None
    }
}
