#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth executor executes transaction in block of data.

use async_trait::async_trait;
use executor::ExecutionResult;
use reth_interfaces::executor::Error;
use reth_primitives::{Address, Block};

pub mod config;
pub mod eth_dao_fork;
/// Executor
pub mod executor;
/// Wrapper around revm database and types
pub mod revm_wrap;

/// An executor capable of executing a block.
#[async_trait]
pub trait BlockExecutor {
    /// Execute block
    /// if `signers` is some, it's length should be equal to block.body's length. Provide `signers`
    /// if you already have it because recovering singer is too expensive.
    fn execute(
        &mut self,
        block: &Block,
        signers: Option<Vec<Address>>,
    ) -> Result<ExecutionResult, Error>;
}
