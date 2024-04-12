//! Traits for execution.

use reth_primitives::U256;

/// A general purpose executor trait that executes on an input and produces an output.
pub trait Executor {
    /// The input type for the executor.
    type Input;
    /// The output type for the executor.
    type Output;
    /// The error type returned by the executor.
    type Error;

    /// Consumes the type and executes the block.
    ///
    /// Returns the output of the block execution.
    fn execute(self, block: Self::Input) -> Result<Self::Output, Self::Error>;
}

/// The output of an ethereum block.
///
/// Contains the receipts of the transactions in the block and the total gas used.
pub struct EthBlockOutput<T> {
    /// All the receipts of the transactions in the block.
    pub receipts: T,
    /// The total gas used by the block.
    pub gas_used: u64,
}

/// A helper trait for ethereum block inputs that consists of a block and the total difficulty.
pub trait EthBlockInput<Block> {
    /// Returns the total difficulty of the block.
    fn total_difficulty(&self) -> U256;

    /// Returns the block to execute.
    fn block(&self) -> &Block;
}

// TODO impl for tuples
