#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Staged syncing primitives for reth.
//!
//! See [Stage] and [Pipeline].
//!
//! # Metrics
//!
//! This library exposes metrics via. the [`metrics`][metrics] crate:
//!
//! - `stage.progress{stage}`: The block number each stage has currently reached.

mod db;
mod error;
mod id;
mod pipeline;
mod stage;
mod util;

#[cfg(test)]
mod test_utils;

/// Implementations of stages.
pub mod stages;

pub use error::*;
pub use id::*;
pub use pipeline::*;
pub use stage::*;

// NOTE: Needed so the link in the module-level rustdoc works.
#[allow(unused_extern_crates)]
extern crate metrics;
