#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Consensus algorithms for Ethereum.
//!
//! # Features
//!
//! - `serde`: Enable serde support for configuration types.
pub mod consensus;
pub mod constants;
pub mod verification;

/// Engine API module.
pub mod engine;

pub use consensus::BeaconConsensus;
pub use reth_interfaces::consensus::Error;
