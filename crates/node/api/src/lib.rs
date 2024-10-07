//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Traits, validation methods, and helper types used to abstract over engine types.
pub use reth_engine_primitives as engine;
pub use reth_engine_primitives::*;

/// Traits and helper types used to abstract over payload types.
pub use reth_payload_primitives as payload;
pub use reth_payload_primitives::*;

/// Traits and helper types used to abstract over EVM methods and types.
pub use reth_evm::{ConfigureEvm, ConfigureEvmEnv, NextBlockEnvAttributes};

pub mod node;
pub use node::*;

// re-export for convenience
pub use reth_node_types::*;
pub use reth_provider::FullProvider;

pub use reth_rpc_eth_api::EthApiTypes;
