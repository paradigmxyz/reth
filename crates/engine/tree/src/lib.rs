//! The [`ChainOrchestrator`] contains the state of the chain and orchestrates the components
//! responsible for advancing the chain.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// #![cfg_attr(not(test), warn(unused_crate_dependencies))]

/// Re-export of the blockchain tree API.
pub use reth_blockchain_tree_api::*;

/// The type that drives the chain forward.
pub mod chain;
/// Support for downloading blocks on demand for live sync.
pub mod download;
/// Engine Api chain handler support.
pub mod engine;
/// Support for managing the pipeline.
pub mod pipeline;
/// Support for interacting with the blockchain tree.
pub mod tree;
