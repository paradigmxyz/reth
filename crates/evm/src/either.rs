//! Helper type that represents one of two possible executor types

use crate::execute::{
    BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput,
    BlockExecutorProvider, Executor,
};
use reth_interfaces::{executor::BlockExecutionError, provider::ProviderError};
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt};
use revm_primitives::db::Database;

/// A type that represents one of two possible executor factories.
#[derive(Debug, Clone)]
pub enum EitherExecutor<A, B> {
    /// The first executor provider variant
    Left(A),
    /// The first executor provider variant
    Right(B),
}

impl<A, B> BlockExecutorProvider for EitherExecutor<A, B>
where
    A: BlockExecutorProvider,
    B: BlockExecutorProvider,
{
    type Executor<DB: Database<Error = ProviderError>> =
        EitherExecutor<A::Executor<DB>, B::Executor<DB>>;
    type BatchExecutor<DB: Database<Error = ProviderError>> =
        EitherExecutor<A::BatchExecutor<DB>, B::BatchExecutor<DB>>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        match self {
            EitherExecutor::Left(a) => EitherExecutor::Left(a.executor(db)),
            EitherExecutor::Right(b) => EitherExecutor::Right(b.executor(db)),
        }
    }

    fn batch_executor<DB>(&self, db: DB, prune_modes: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        match self {
            EitherExecutor::Left(a) => EitherExecutor::Left(a.batch_executor(db, prune_modes)),
            EitherExecutor::Right(b) => EitherExecutor::Right(b.batch_executor(db, prune_modes)),
        }
    }
}

impl<A, B, DB> Executor<DB> for EitherExecutor<A, B>
where
    A: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error: Into<BlockExecutionError>,
    >,
    B: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error: Into<BlockExecutionError>,
    >,
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        match self {
            EitherExecutor::Left(a) => a.execute(input).map_err(Into::into),
            EitherExecutor::Right(b) => b.execute(input).map_err(Into::into),
        }
    }
}

impl<A, B, DB> BatchExecutor<DB> for EitherExecutor<A, B>
where
    A: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BatchBlockExecutionOutput,
        Error: Into<BlockExecutionError>,
    >,
    B: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BatchBlockExecutionOutput,
        Error: Into<BlockExecutionError>,
    >,
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BatchBlockExecutionOutput;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        match self {
            EitherExecutor::Left(a) => a.execute_one(input).map_err(Into::into),
            EitherExecutor::Right(b) => b.execute_one(input).map_err(Into::into),
        }
    }

    fn finalize(self) -> Self::Output {
        match self {
            EitherExecutor::Left(a) => a.finalize(),
            EitherExecutor::Right(b) => b.finalize(),
        }
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        match self {
            EitherExecutor::Left(a) => a.set_tip(tip),
            EitherExecutor::Right(b) => b.set_tip(tip),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        match self {
            EitherExecutor::Left(a) => a.size_hint(),
            EitherExecutor::Right(b) => b.size_hint(),
        }
    }
}
