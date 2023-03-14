#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth executor executes transaction in block of data.

pub mod eth_dao_fork;
pub mod substate;

/// Execution result types.
pub use reth_provider::execution_result;
pub mod blockchain_tree;
/// Executor
pub mod executor;

/// ExecutorFactory impl
pub mod factory;
pub use factory::Factory;
