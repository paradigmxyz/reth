#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Collection of metrics utilities.

/// Metrics derive macro.
pub use reth_metrics_derive::Metrics;

/// Implementation of common metric utilities.
#[cfg(feature = "common")]
pub mod common;

/// Re-export core metrics crate.
pub use metrics;
