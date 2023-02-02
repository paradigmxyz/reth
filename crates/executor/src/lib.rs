#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth executor executes transaction in block of data.

use async_trait::async_trait;
use execution_result::ExecutionResult;
use reth_interfaces::executor::Error;
use reth_primitives::{Address, Block};

pub mod config;
pub mod eth_dao_fork;

/// Execution result types
pub mod execution_result;
/// Executor
pub mod executor;
/// Wrapper around revm database and types
pub mod revm_wrap;

/// An executor capable of executing a block.
#[async_trait]
pub trait BlockExecutor {
    /// Execute a block.
    ///
    /// The number of `senders` should be equal to the number of transactions in the block.
    fn execute(
        &mut self,
        block: &Block,
        senders: Option<Vec<Address>>,
    ) -> Result<ExecutionResult, Error>;
}
