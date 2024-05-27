//! A collection of shared traits and error types used in Reth.
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Storage error types
pub use reth_storage_errors::{db, provider};

/// Block Execution traits.
pub use reth_execution_errors as executor;

/// Possible errors when interacting with the chain.
mod error;
pub use error::{RethError, RethResult};

/// P2P traits.
pub use reth_network_p2p as p2p;

/// Trie error
pub use reth_execution_errors::trie;

/// Syncing related traits.
pub use reth_network_p2p::sync;

/// BlockchainTree related traits.
pub use reth_blockchain_tree_api as blockchain_tree;

/// Common test helpers for mocking out Consensus, Downloaders and Header Clients.
#[cfg(feature = "test-utils")]
pub use reth_network_p2p::test_utils;
