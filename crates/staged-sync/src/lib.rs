#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Puts together all the Reth stages in a unified abstraction

pub mod config;
pub use config::Config;

pub mod utils;

#[cfg(any(test, feature = "test-utils"))]
/// Common helpers for integration testing.
pub mod test_utils;
