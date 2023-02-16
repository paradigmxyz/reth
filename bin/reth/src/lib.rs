#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Rust Ethereum (reth) binary executable.

pub mod args;
pub mod chain;
pub mod cli;
pub mod db;
pub mod dirs;
pub mod dump_stage;
pub mod node;
pub mod p2p;
pub mod prometheus_exporter;
pub mod runner;
pub mod stage;
pub mod test_eth_chain;
pub mod test_vectors;
