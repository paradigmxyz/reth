#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
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
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

pub mod database;
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
pub use traits::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};
