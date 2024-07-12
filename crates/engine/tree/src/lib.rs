//! This crate includes the core components for advancing a reth chain.
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
// #![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(missing_docs, dead_code, missing_debug_implementations, unused_variables)] // TODO rm

/// Re-export of the blockchain tree API.
pub use reth_blockchain_tree_api::*;

/// Support for backfill sync mode.
pub mod backfill;
/// The type that drives the chain forward.
pub mod chain;
/// The background writer service for batch db writes.
pub mod database;
/// Support for downloading blocks on demand for live sync.
pub mod download;
/// Engine Api chain handler support.
pub mod engine;
/// Metrics support.
pub mod metrics;
/// The background writer service, coordinating the static file and database services.
pub mod persistence;
/// The background writer service for static file writes.
pub mod static_files;
/// Support for interacting with the blockchain tree.
pub mod tree;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
