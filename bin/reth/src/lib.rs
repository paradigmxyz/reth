#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
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
pub mod drop_stage;
pub mod dump_stage;
pub mod node;
pub mod p2p;
pub mod prometheus_exporter;
pub mod runner;
pub mod stage;
pub mod test_eth_chain;
pub mod test_vectors;
pub mod utils;

#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, clap::ValueEnum)]
enum StageEnum {
    Headers,
    Bodies,
    Senders,
    Execution,
    Merkle,
}
