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

/// Cache database that reads from an underlying [`DatabaseRef`].
/// Database adapters for payload building.
pub mod cached;

/// A marker that can be used to cancel execution.
pub mod cancelled;

/// Contains glue code for integrating reth database into revm's [Database].
pub mod database;

pub use revm_database as db;
pub use revm_inspector as inspector;

/// Common test helpers
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

// Convenience re-exports.
pub use revm::{self, database::State, *};

/// Either type for flexible usage of different database types in the same context.
pub mod either;

/// Helper types for execution witness generation.
#[cfg(feature = "witness")]
pub mod witness;
