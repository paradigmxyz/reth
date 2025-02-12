//! A no operation block executor implementation.

use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionResult, ExecutionOutcome};
use reth_primitives::{NodePrimitives, RecoveredBlock};

use crate::{
    execute::{BatchExecutor, BlockExecutorProvider, Executor},
    system_calls::OnStateHook,
    Database,
};

const UNAVAILABLE_FOR_NOOP: &str = "execution unavailable for noop";

/// A [`BlockExecutorProvider`] implementation that does nothing.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct NoopBlockExecutorProvider<P>(core::marker::PhantomData<P>);

impl<P: NodePrimitives> BlockExecutorProvider for NoopBlockExecutorProvider<P> {
    type Primitives = P;

    type Executor<DB: Database> = Self;

    type BatchExecutor<DB: Database> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database,
    {
        Self::default()
    }

    fn batch_executor<DB>(&self, _: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database,
    {
        Self::default()
    }
}

impl<DB: Database, P: NodePrimitives> Executor<DB> for NoopBlockExecutorProvider<P> {
    type Primitives = P;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn execute_one_with_state_hook<F>(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
        _state_hook: F,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn into_state(self) -> revm::db::State<DB> {
        unreachable!()
    }

    fn size_hint(&self) -> usize {
        0
    }
}

impl<DB, P: NodePrimitives> BatchExecutor<DB> for NoopBlockExecutorProvider<P> {
    type Input<'a> = &'a RecoveredBlock<P::Block>;
    type Output = ExecutionOutcome<P::Receipt>;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn finalize(self) -> Self::Output {
        unreachable!()
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }
}
