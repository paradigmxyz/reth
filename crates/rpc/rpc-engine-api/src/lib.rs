//! The implementation of Engine API.
//! [Read more](https://github.com/ethereum/execution-apis/tree/main/src/engine).

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// The Engine API implementation.
mod engine_api;

/// The Engine API message type.
mod message;

/// Engine API error.
mod error;

/// Engine API metrics.
mod metrics;

pub use engine_api::{EngineApi, EngineApiSender};
pub use error::*;
pub use message::EngineApiMessageVersion;

// re-export server trait for convenience
pub use reth_rpc_api::EngineApiServer;

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    // silence unused import warning
    use alloy_rlp as _;
}
