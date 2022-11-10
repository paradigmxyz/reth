#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth consensus.
pub mod config;
pub mod consensus;
pub mod verification;

/// Helper function for calculating Merkle proofs and hashes
pub mod proofs;

pub use config::Config;
pub use consensus::EthConsensus;
pub use reth_interfaces::consensus::Error;
