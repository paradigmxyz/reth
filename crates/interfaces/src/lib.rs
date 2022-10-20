#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth interface bindings

/// Block Execution traits.
pub mod executor;

/// Consensus traits.
pub mod consensus;

/// Database traits.
pub mod db;
/// Traits that provide chain access.
pub mod provider;

/// Possible errors when interacting with the chain.
mod error;

pub use error::{Error, Result};
