//! Helper type that represents one of two possible executor types

use crate::{
    execute::{BatchExecutor, BlockExecutorProvider, Executor},
    system_calls::OnStateHook,
    Database,
};

// re-export Either
pub use futures_util::future::Either;
use revm::State;

impl<A, B> BlockExecutorProvider for Either<A, B>
where
    A: BlockExecutorProvider,
    B: BlockExecutorProvider<Primitives = A::Primitives>,
{
    type Primitives = A::Primitives;

    type Executor<DB: Database> = Either<A::Executor<DB>, B::Executor<DB>>;

    type BatchExecutor<DB: Database> = Either<A::BatchExecutor<DB>, B::BatchExecutor<DB>>;

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database,
    {
        match self {
            Self::Left(a) => Either::Left(a.executor(db)),
            Self::Right(b) => Either::Right(b.executor(db)),
        }
    }

    fn batch_executor<DB>(&self, db: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database,
    {
        match self {
            Self::Left(a) => Either::Left(a.batch_executor(db)),
            Self::Right(b) => Either::Right(b.batch_executor(db)),
        }
    }
}

impl<A, B, DB> Executor<DB> for Either<A, B>
where
    A: Executor<DB>,
    B: for<'a> Executor<DB, Input<'a> = A::Input<'a>, Output = A::Output, Error = A::Error>,
    DB: Database,
{
    type Input<'a> = A::Input<'a>;
    type Output = A::Output;
    type Error = A::Error;

    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        match self {
            Self::Left(a) => a.execute(input),
            Self::Right(b) => b.execute(input),
        }
    }

    fn execute_with_state_closure<F>(
        self,
        input: Self::Input<'_>,
        witness: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&State<DB>),
    {
        match self {
            Self::Left(a) => a.execute_with_state_closure(input, witness),
            Self::Right(b) => b.execute_with_state_closure(input, witness),
        }
    }

    fn execute_with_state_hook<F>(
        self,
        input: Self::Input<'_>,
        state_hook: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: OnStateHook + 'static,
    {
        match self {
            Self::Left(a) => a.execute_with_state_hook(input, state_hook),
            Self::Right(b) => b.execute_with_state_hook(input, state_hook),
        }
    }
}

impl<A, B, DB> BatchExecutor<DB> for Either<A, B>
where
    A: BatchExecutor<DB>,
    B: for<'a> BatchExecutor<DB, Input<'a> = A::Input<'a>, Output = A::Output, Error = A::Error>,
    DB: Database,
{
    type Input<'a> = A::Input<'a>;
    type Output = A::Output;
    type Error = A::Error;

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

    fn size_hint(&self) -> Option<usize> {
        match self {
            Self::Left(a) => a.size_hint(),
            Self::Right(b) => b.size_hint(),
        }
    }
}
