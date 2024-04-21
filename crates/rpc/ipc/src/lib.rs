//! Reth IPC transport implementation
//!
//! ## Feature Flags
//!
//! - `client`: Enables JSON-RPC client support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[cfg(unix)]
pub mod client;
pub mod server;

/// Json codec implementation
pub mod stream_codec;
