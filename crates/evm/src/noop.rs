//! A no operation block executor implementation.

use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_primitives::{NodePrimitives, RecoveredBlock};
use revm::State;

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

impl<DB, P: NodePrimitives> Executor<DB> for NoopBlockExecutorProvider<P> {
    type Input<'a> = &'a RecoveredBlock<P::Block>;
    type Output = BlockExecutionOutput<P::Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        Err(BlockExecutionError::msg(UNAVAILABLE_FOR_NOOP))
    }

    fn execute_with_state_closure<F>(
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
