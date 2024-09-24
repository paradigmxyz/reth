//! Reth RPC type definitions.
//!
//! Provides all relevant types for the various RPC endpoints, grouped by namespace.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#[allow(hidden_glob_reexports)]
mod eth;

// Ethereum specific rpc types related to typed transaction requests and the engine API.
#[cfg(feature = "jsonrpsee-types")]
pub use eth::error::ToRpcError;
#[cfg(feature = "jsonrpsee-types")]
pub use eth::{
    engine,
    engine::{
        ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, PayloadError,
    },
};
