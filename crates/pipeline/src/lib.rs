#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! The `reth-pipeline` contains a Pipeline and a builder pattern for
//! it for queueing up stages and running the node or parts of it.

/// The pipeline of stages which are executed sequentially
pub mod pipeline;

/// The possible control flow return values from the pipeline
pub mod ctrl;

/// Events emitted by the pipeline through its lifecycle
pub mod event;

/// Tracks the pipeline's progress
pub mod state;

pub use event::*;
