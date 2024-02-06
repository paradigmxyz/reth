//! Standalone crate for Reth configuration and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Exports optimism-specific types that implement traits in [reth_node_api].
#[cfg(feature = "optimism")]
pub mod optimism;
#[cfg(feature = "optimism")]
pub use optimism::{OptimismEngineTypes, OptimismEvmConfig};

/// Node event hooks.
pub mod hooks;

/// Support for configuring the higher level node types.
pub mod node;

/// Support for configuring the components of a node.
pub mod components;

mod builder;
mod handle;
pub mod rpc;

pub mod provider;

pub use builder::*;
pub use handle::NodeHandle;