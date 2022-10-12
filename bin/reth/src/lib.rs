#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Rust Ethereum (reth) binary executable.

/// CLI definition and entrypoint
pub mod cli;

/// Utility functions.
pub mod util;

/// Command for executing Ethereum blockchain tests
pub mod test_eth_chain;
