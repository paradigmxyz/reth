//! Traits for execution.

use reth_primitives::{BlockNumber, Receipts, U256};
use revm::db::BundleState;
use revm_primitives::db::{Database, DatabaseCommit};

/// A general purpose executor trait that executes on an input and produces an output.
pub trait Executor {
    /// The input type for the executor.
    type Input<'a>;
    /// The output type for the executor.
    type Output;
    /// The error type returned by the executor.
    type Error;

    /// Consumes the type and executes the block.
    ///
    /// Returns the output of the block execution.
    fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error>;
}

/// An executor that can execute multiple blocks in a row and keep track of the state over the
/// entire batch.
pub trait BatchExecutor {
    /// Input
    type Input<'a>;
    /// The output type for the executor.
    type Output;
    type Error;

    /// Executes the next block in the batch and update the state internally.
    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error>;

    /// Finishes the batch and return the final state.
    fn finalize(self) -> Self::Output;
}

pub struct BatchBlockOutput {
    /// The size hint of the batch's tracked state.
    pub size_hint: Option<usize>,
}

/// The output of an ethereum block.
///
/// Contains the receipts of the transactions in the block and the total gas used.
#[derive(Debug)]
pub struct EthBlockOutput<T> {
    /// The changed state of the block after execution.
    pub state: BundleState,
    /// All the receipts of the transactions in the block.
    pub receipts: T,
    /// The total gas used by the block.
    pub gas_used: u64,
}

/// The final output of a batch of blocks.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct EthBatchOutput<T> {
    /// Bundle state with reverts.
    pub bundle: BundleState,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    pub receipts: T,
    /// First block of bundle state.
    pub first_block: BlockNumber,
}

/// A helper type for ethereum block inputs that consists of a block and the total difficulty.
#[derive(Debug)]
pub struct EthBlockExecutionInput<'a, Block> {
    /// The block to execute.
    pub block: &'a Block,
    /// The total difficulty of the block.
    pub total_difficulty: U256,
}

/// A type that can create a new executor.
pub trait ExecutorProvider: Send + Sync + Clone {
    /// Creates a new batch executor
    fn batch_executor<DB>(&self, db: DB) -> impl BatchExecutor
    where
        DB: Database + DatabaseCommit;

    // Returns a new executor for single block execution.
    fn executor<DB>(&self, db: DB) -> impl Executor
    where
        DB: Database + DatabaseCommit;
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use revm::db::{CacheDB, EmptyDB};

    use super::*;

    #[derive(Clone, Default)]
    struct TestExecutorProvider;

    impl ExecutorProvider for TestExecutorProvider {
        fn batch_executor<DB>(&self, db: DB) -> impl BatchExecutor
        where
            DB: Database + DatabaseCommit,
        {
            todo!()
        }

        fn executor<DB>(&self, db: DB) -> impl Executor
        where
            DB: Database + DatabaseCommit,
        {
            TestExecutor(PhantomData)
        }
    }

    struct TestExecutor<DB>(PhantomData<DB>);

    impl<DB> Executor for TestExecutor<DB> {
        type Input<'a> = &'a str;
        type Output = ();
        type Error = ();

        fn execute(self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
            Ok(())
        }
    }

    impl<DB> BatchExecutor for TestExecutor<DB> {
        type Input<'a> = &'a str;
        type Output = ();
        type Error = ();

        fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error> {
            Ok(BatchBlockOutput { size_hint: None })
        }

        fn finalize(self) -> Self::Output {
            ()
        }
    }

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider::default();
        let db = CacheDB::<EmptyDB>::default();
        let mut executor = provider.executor(db);
        executor.execute("test").unwrap();
    }
}
