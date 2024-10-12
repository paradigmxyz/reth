//! Revm utils and implementations specific to reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// Contains glue code for integrating reth database into revm's [Database].
pub mod database;

pub mod batch;

/// State changes that are not related to transactions.
pub mod state_change;

/// Common test helpers
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

// Convenience re-exports.
pub use revm::{self, *};
