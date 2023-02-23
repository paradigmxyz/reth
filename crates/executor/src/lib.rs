#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth executor executes transaction in block of data.

pub mod eth_dao_fork;

/// Execution result types
pub mod execution_result;
/// Executor
pub mod executor;
