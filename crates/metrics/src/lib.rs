//! Collection of metrics utilities.
//!
//! ## Feature Flags
//!
//! - `common`: Common metrics utilities, such as wrappers around tokio senders and receivers. Pulls
//!   in `tokio`.
//! - `fuzz`: Derive inert metric handles without registering, describing, or caching metrics.
//!   Direct calls to the `metrics` facade are unaffected.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

// The native derive remains a dependency in fuzz builds, but is not used there.
#[cfg(feature = "fuzz")]
use metrics_derive as _;

/// Metrics derive macro.
#[cfg(not(feature = "fuzz"))]
pub use metrics_derive::Metrics;

/// Fuzz-only metrics derive macro.
#[cfg(feature = "fuzz")]
pub use reth_metrics_derive::Metrics;

/// Implementation of common metric utilities.
#[cfg(feature = "common")]
pub mod common;

/// Re-export core metrics crate.
pub use metrics;

/// Per-thread resource usage measurement.
pub mod thread;
