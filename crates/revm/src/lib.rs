//! Revm utils and implementations specific to reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Re-export everything
pub use revm::{self, *};

/// Re-export for convenience.
pub use reth_revm_inspectors::*;

/// Re-export for convenience.
pub use reth_revm_executor::*;

/// Re-export revm database integration.
pub mod database {
    pub use reth_revm_database::*;
}

/// Re-export parallel library if the feature is enabled.
#[cfg(feature = "parallel")]
pub mod parallel {
    pub use reth_revm_parallel::*;
}
