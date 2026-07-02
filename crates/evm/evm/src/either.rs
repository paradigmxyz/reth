//! Helper type that represents one of two possible executor types

use crate::execute::Executor;
use alloc::vec::Vec;

// re-export Either
pub use futures_util::future::Either;
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, ExecutionOutcome};
use reth_primitives_traits::{NodePrimitives, ReceiptTy, RecoveredBlock};

impl<A, B> Executor for Either<A, B>
where
    A: Executor,
    B: Executor<Primitives = A::Primitives, Error = A::Error>,
{
    type Primitives = A::Primitives;
    type Error = A::Error;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        match self {
            Self::Left(a) => a.execute_one(block),
            Self::Right(b) => b.execute_one(block),
        }
    }

    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        match self {
            Self::Left(a) => a.execute(block),
            Self::Right(b) => b.execute(block),
        }
    }

    fn size_hint(&self) -> usize {
        match self {
            Self::Left(a) => a.size_hint(),
            Self::Right(b) => b.size_hint(),
        }
    }

    fn into_execution_outcome(
        self,
        first_block: u64,
        results: Vec<BlockExecutionResult<ReceiptTy<Self::Primitives>>>,
    ) -> ExecutionOutcome<ReceiptTy<Self::Primitives>> {
        match self {
            Self::Left(a) => a.into_execution_outcome(first_block, results),
            Self::Right(b) => b.into_execution_outcome(first_block, results),
        }
    }

    fn take_bal(&mut self) -> Option<alloy_primitives::Bytes> {
        match self {
            Self::Left(a) => a.take_bal(),
            Self::Right(b) => b.take_bal(),
        }
    }
}
