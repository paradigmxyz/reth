//! Helper type that represents one of two possible executor types

use crate::execute::{
    BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput,
    BlockExecutorProvider, Executor,
};
use reth_interfaces::{executor::BlockExecutionError, provider::ProviderError};
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt};
use revm_primitives::db::Database;

// re-export Either
pub use futures_util::future::Either;

impl<A, B> BlockExecutorProvider for Either<A, B>
where
    A: BlockExecutorProvider,
    B: BlockExecutorProvider,
{
    type Executor<DB: Database<Error = ProviderError>> = Either<A::Executor<DB>, B::Executor<DB>>;
    type BatchExecutor<DB: Database<Error = ProviderError>> =
        Either<A::BatchExecutor<DB>, B::BatchExecutor<DB>>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        match self {
            Either::Left(a) => Either::Left(a.executor(db)),
            Either::Right(b) => Either::Right(b.executor(db)),
        }
    }

    fn batch_executor<DB>(&self, db: DB, prune_modes: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        match self {
            Either::Left(a) => Either::Left(a.batch_executor(db, prune_modes)),
            Either::Right(b) => Either::Right(b.batch_executor(db, prune_modes)),
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
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        match self {
            Either::Left(a) => a.execute(input),
            Either::Right(b) => b.execute(input),
        }
    }
}

impl<A, B, DB> BatchExecutor<DB> for Either<A, B>
where
    A: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BatchBlockExecutionOutput,
        Error = BlockExecutionError,
    >,
    B: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BatchBlockExecutionOutput,
        Error = BlockExecutionError,
    >,
    DB: Database<Error = ProviderError>,
{
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BatchBlockExecutionOutput;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error> {
        match self {
            Either::Left(a) => a.execute_one(input),
            Either::Right(b) => b.execute_one(input),
        }
    }

    fn finalize(self) -> Self::Output {
        match self {
            Either::Left(a) => a.finalize(),
            Either::Right(b) => b.finalize(),
        }
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        match self {
            Either::Left(a) => a.set_tip(tip),
            Either::Right(b) => b.set_tip(tip),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        match self {
            Either::Left(a) => a.size_hint(),
            Either::Right(b) => b.size_hint(),
        }
    }
}
