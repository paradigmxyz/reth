#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! A collection of shared traits and error types used in Reth.
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

/// Consensus traits.
pub mod consensus;

/// Database error
pub mod db;

/// Block Execution traits.
pub mod executor;

/// Possible errors when interacting with the chain.
mod error;
pub use error::{Error, Result};

/// P2P traits.
pub mod p2p;

/// Provider error
pub mod provider;

/// Syncing related traits.
pub mod sync;

/// BlockchainTree related traits.
pub mod blockchain_tree;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking out Consensus, Downloaders and Header Clients.
pub mod test_utils;
