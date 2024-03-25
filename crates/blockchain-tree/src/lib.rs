//! Implementation of a tree-like structure for blockchains.
//!
//! The [BlockchainTree] can validate, execute, and revert blocks in multiple competing sidechains.
//! This structure is used for Reth's sync mode at the tip instead of the pipeline, and is the
//! primary executor and validator of payloads sent from the consensus layer.
//!
//! Blocks and their resulting state transitions are kept in-memory until they are persisted.
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

pub mod blockchain_tree;
pub use blockchain_tree::BlockchainTree;

pub mod block_indices;
pub use block_indices::BlockIndices;

pub mod chain;
pub use chain::AppendableChain;

pub mod config;
pub use config::BlockchainTreeConfig;

pub mod externals;
pub use externals::TreeExternals;

pub mod shareable;
pub use shareable::ShareableBlockchainTree;

mod bundle;
pub use bundle::{BundleStateData, BundleStateDataRef};

/// Buffer of not executed blocks.
pub mod block_buffer;
mod canonical_chain;

/// Common blockchain tree metrics.
pub mod metrics;

pub use block_buffer::BlockBuffer;

/// Implementation of Tree traits that does nothing.
pub mod noop;

mod state;

use aquamarine as _;
