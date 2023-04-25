#![warn(missing_docs)]
#![deny(
    unused_must_use,
    rust_2018_idioms,
    rustdoc::broken_intra_doc_links,
    unused_crate_dependencies
)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This trait implements the [PayloadBuilderService] responsible for managing payload jobs.
//!
//! It Defines the abstractions to create and update payloads:
//!   - [PayloadJobGenerator]: a type that knows how to create new jobs for creating payloads based
//!     on [PayloadAttributes](reth_rpc_types::engine::PayloadAttributes).
//!   - [PayloadJob]: a type that can yields (better) payloads over time.

pub mod error;
mod metrics;
mod payload;
mod service;
mod traits;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

pub use payload::{BuiltPayload, PayloadBuilderAttributes};
pub use reth_rpc_types::engine::PayloadId;
pub use service::{PayloadBuilderHandle, PayloadBuilderService, PayloadStore};
pub use traits::{PayloadJob, PayloadJobGenerator};
