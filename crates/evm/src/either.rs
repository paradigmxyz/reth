//! Helper type that represents one of two possible executor types

use core::fmt::Display;

use crate::{
    execute::{BatchExecutor, BlockExecutorProvider, Executor},
    system_calls::OnStateHook,
};
use alloy_primitives::BlockNumber;
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{BlockExecutionInput, BlockExecutionOutput, ExecutionOutcome};
use reth_primitives::{BlockWithSenders, Receipt};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderError;
use revm_primitives::db::Database;

// re-export Either
pub use futures_util::future::Either;
use revm::State;

impl<A, B> BlockExecutorProvider for Either<A, B>
where
    A: BlockExecutorProvider,
    B: BlockExecutorProvider,
{
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> =
        Either<A::Executor<DB>, B::Executor<DB>>;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> =
        Either<A::BatchExecutor<DB>, B::BatchExecutor<DB>>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        match self {
            Self::Left(a) => Either::Left(a.executor(db)),
            Self::Right(b) => Either::Right(b.executor(db)),
        }
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        match self {
            Self::Left(a) => Either::Left(a.batch_executor(db)),
            Self::Right(b) => Either::Right(b.batch_executor(db)),
        }
    }
}

impl<A, B, DB> Executor<DB> for Either<A, B>
where
    A: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error = BlockExecutionError,
    >,
    B: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error = BlockExecutionError,
    >,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        match self {
            Self::Left(a) => a.execute(input),
            Self::Right(b) => b.execute(input),
        }
    }

    fn execute_with_state_witness<F>(
        self,
        input: Self::Input<'_>,
        witness: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        match self {
            Self::Left(a) => a.execute_with_state_witness(input, witness),
            Self::Right(b) => b.execute_with_state_witness(input, witness),
        }
    }

    fn execute_with_state_hook<F>(
        self,
        input: Self::Input<'_>,
        state_hook: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook,
    {
        match self {
            Self::Left(a) => a.execute_with_state_hook(input, state_hook),
            Self::Right(b) => b.execute_with_state_hook(input, state_hook),
        }
    }
}

impl<A, B, DB> BatchExecutor<DB> for Either<A, B>
where
    A: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = ExecutionOutcome,
        Error = BlockExecutionError,
    >,
    B: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = ExecutionOutcome,
        Error = BlockExecutionError,
    >,
    DB: Database<Error: Into<ProviderError> + Display>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        match self {
            Self::Left(a) => a.execute_and_verify_one(input),
            Self::Right(b) => b.execute_and_verify_one(input),
        }
    }

    fn finalize(self) -> Self::Output {
        match self {
            Self::Left(a) => a.finalize(),
            Self::Right(b) => b.finalize(),
        }
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        match self {
            Self::Left(a) => a.set_tip(tip),
            Self::Right(b) => b.set_tip(tip),
        }
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        match self {
            Self::Left(a) => a.set_prune_modes(prune_modes),
            Self::Right(b) => b.set_prune_modes(prune_modes),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        match self {
            Self::Left(a) => a.size_hint(),
            Self::Right(b) => b.size_hint(),
        }
    }
}
