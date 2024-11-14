//! Standalone crate for Reth configuration and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Node event hooks.
pub mod hooks;

/// Support for configuring the higher level node types.
pub mod node;
pub use node::*;

/// Support for configuring the components of a node.
pub mod components;
pub use components::{NodeComponents, NodeComponentsBuilder};

mod builder;
pub use builder::{add_ons::AddOns, *};

mod launch;
pub use launch::{engine::EngineNodeLauncher, *};

/// Temporarily re-export engine tree config.
pub use reth_engine_tree::tree::config as engine_tree_config;

mod handle;
pub use handle::NodeHandle;

pub mod rpc;

pub mod setup;

/// Support for installing the ExExs (execution extensions) in a node.
pub mod exex;

/// Re-export the core configuration traits.
pub use reth_node_core::cli::config::{
    PayloadBuilderConfig, RethNetworkConfig, RethTransactionPoolConfig,
};

// re-export the core config for convenience
pub use reth_node_core::node_config::NodeConfig;

// re-export API types for convenience
pub use reth_node_api::*;

use aquamarine as _;

use reth_rpc as _;
