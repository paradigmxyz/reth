#![warn(missing_debug_implementations, missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth RPC type definitions
//!
//! Provides all relevant types for the various RPC endpoints, grouped by namespace.

mod admin;
mod eth;
mod rpc;

pub use admin::*;
pub use eth::*;
pub use rpc::*;
