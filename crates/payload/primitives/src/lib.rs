//! This crate defines abstractions to create and update payloads (blocks):
//! - [`PayloadJobGenerator`]: a type that knows how to create new jobs for creating payloads based
//!   on `reth_rpc_types::engine::PayloadAttributes`.
//! - [`PayloadJob`]: a type that yields (better) payloads over time.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod error;
mod traits;

pub use traits::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};
