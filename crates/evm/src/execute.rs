//! Traits for execution.

use reth_interfaces::{executor::BlockExecutionError, provider::ProviderError};
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt, Receipts, U256};
use revm::db::BundleState;
use revm_primitives::db::Database;

/// A general purpose executor trait that executes on an input (e.g. blocks) and produces an output
/// (e.g. state changes and receipts).
pub trait Executor<DB> {
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

/// A general purpose executor that can execute multiple inputs in sequence and keep track of the
/// state over the entire batch.
pub trait BatchExecutor<DB> {
    /// The input type for the executor.
    type Input<'a>;
    /// The output type for the executor.
    type Output;
    /// The error type returned by the executor.
    type Error;

    /// Executes the next block in the batch and update the state internally.
    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<(), Self::Error>;

    /// Executes multiple inputs in the batch and update the state internally.
    fn execute_many<'a, I>(&mut self, inputs: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Input<'a>>,
    {
        for input in inputs {
            self.execute_one(input)?;
        }
        Ok(())
    }

    /// Executes the entire batch and return the final state.
    fn execute_batch<'a, I>(mut self, batch: I) -> Result<Self::Output, Self::Error>
    where
        I: IntoIterator<Item = Self::Input<'a>>,
        Self: Sized,
    {
        self.execute_many(batch)?;
        Ok(self.finalize())
    }

    /// Finishes the batch and return the final state.
    fn finalize(self) -> Self::Output;

    /// Set the expected tip of the batch.
    ///
    /// This can be used to optimize state pruning during execution.
    fn set_tip(&mut self, tip: BlockNumber);

    /// The size hint of the batch's tracked state size.
    ///
    /// This is used to optimize DB commits depending on the size of the state.
    fn size_hint(&self) -> Option<usize>;
}

/// The output of an ethereum block.
///
/// Contains the state changes, transaction receipts, and total gas used in the block.
///
/// TODO(mattsse): combine with BundleStateWithReceipts
#[derive(Debug)]
pub struct BlockExecutionOutput<T> {
    /// The changed state of the block after execution.
    pub state: BundleState,
    /// All the receipts of the transactions in the block.
    pub receipts: Vec<T>,
    /// The total gas used by the block.
    pub gas_used: u64,
}

/// The output of a batch of ethereum blocks.
#[derive(Debug)]
pub struct BatchBlockExecutionOutput {
    /// Bundle state with reverts.
    pub bundle: BundleState,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    pub receipts: Receipts,
    /// First block of bundle state.
    pub first_block: BlockNumber,
}

impl BatchBlockExecutionOutput {
    /// Create Bundle State.
    pub fn new(bundle: BundleState, receipts: Receipts, first_block: BlockNumber) -> Self {
        Self { bundle, receipts, first_block }
    }
}

/// A helper type for ethereum block inputs that consists of a block and the total difficulty.
#[derive(Debug)]
pub struct BlockExecutionInput<'a, Block> {
    /// The block to execute.
    pub block: &'a Block,
    /// The total difficulty of the block.
    pub total_difficulty: U256,
}

impl<'a, Block> BlockExecutionInput<'a, Block> {
    /// Creates a new input.
    pub fn new(block: &'a Block, total_difficulty: U256) -> Self {
        Self { block, total_difficulty }
    }
}

impl<'a, Block> From<(&'a Block, U256)> for BlockExecutionInput<'a, Block> {
    fn from((block, total_difficulty): (&'a Block, U256)) -> Self {
        Self::new(block, total_difficulty)
    }
}

/// A type that can create a new executor for block execution.
pub trait BlockExecutorProvider: Send + Sync + Clone + Unpin + 'static {
    /// An executor that can execute a single block given a database.
    ///
    /// # Verification
    ///
    /// The on [Executor::execute] the executor is expected to validate the execution output of the
    /// input, this includes:
    /// - Cumulative gas used must match the input's gas used.
    /// - Receipts must match the input's receipts root.
    ///
    /// It is not expected to validate the state trie root, this must be done by the caller using
    /// the returned state.
    type Executor<DB: Database<Error = ProviderError>>: for<'a> Executor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        Output = BlockExecutionOutput<Receipt>,
        Error = BlockExecutionError,
    >;

    /// An executor that can execute a batch of blocks given a database.
    type BatchExecutor<DB: Database<Error = ProviderError>>: for<'a> BatchExecutor<
        DB,
        Input<'a> = BlockExecutionInput<'a, BlockWithSenders>,
        // TODO: change to bundle state with receipts
        Output = BatchBlockExecutionOutput,
        Error = BlockExecutionError,
    >;

    /// Creates a new executor for single block execution.
    ///
    /// This is used to execute a single block and get the changed state.
    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>;

    /// Creates a new batch executor with the given database and pruning modes.
    ///
    /// Batch executor is used to execute multiple blocks in sequence and keep track of the state
    /// during historical sync which involves executing multiple blocks in sequence.
    ///
    /// The pruning modes are used to determine which parts of the state should be kept during
    /// execution.
    fn batch_executor<DB>(&self, db: DB, prune_modes: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::Block;
    use revm::db::{CacheDB, EmptyDBTyped};
    use std::marker::PhantomData;

    #[derive(Clone, Default)]
    struct TestExecutorProvider;

    impl BlockExecutorProvider for TestExecutorProvider {
        type Executor<DB: Database<Error = ProviderError>> = TestExecutor<DB>;
        type BatchExecutor<DB: Database<Error = ProviderError>> = TestExecutor<DB>;

        fn executor<DB>(&self, _db: DB) -> Self::Executor<DB>
        where
            DB: Database<Error = ProviderError>,
        {
            TestExecutor(PhantomData)
        }

        fn batch_executor<DB>(&self, _db: DB, _prune_modes: PruneModes) -> Self::BatchExecutor<DB>
        where
            DB: Database<Error = ProviderError>,
        {
            TestExecutor(PhantomData)
        }
    }

    struct TestExecutor<DB>(PhantomData<DB>);

    impl<DB> Executor<DB> for TestExecutor<DB> {
        type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
        type Output = BlockExecutionOutput<Receipt>;
        type Error = BlockExecutionError;

        fn execute(self, _input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
            Err(BlockExecutionError::UnavailableForTest)
        }
    }

    impl<DB> BatchExecutor<DB> for TestExecutor<DB> {
        type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
        type Output = BatchBlockExecutionOutput;
        type Error = BlockExecutionError;

        fn execute_one(&mut self, _input: Self::Input<'_>) -> Result<(), Self::Error> {
            Ok(())
        }

        fn finalize(self) -> Self::Output {
            todo!()
        }

        fn set_tip(&mut self, _tip: BlockNumber) {
            todo!()
        }

        fn size_hint(&self) -> Option<usize> {
            None
        }
    }

    #[test]
    fn test_provider() {
        let provider = TestExecutorProvider;
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();
        let executor = provider.executor(db);
        let block =
            Block { header: Default::default(), body: vec![], ommers: vec![], withdrawals: None };
        let block = BlockWithSenders::new(block, Default::default()).unwrap();
        let _ = executor.execute(BlockExecutionInput::new(&block, U256::ZERO));
    }
}
