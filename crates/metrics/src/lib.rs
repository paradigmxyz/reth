#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Collection of metrics utilities.
//!
//! ## Feature Flags
//!
//! - `common`: Common metrics utilities, such as wrappers around tokio senders and receivers. Pulls
//!   in `tokio`.

/// Metrics derive macro.
pub use reth_metrics_derive::Metrics;

/// Implementation of common metric utilities.
#[cfg(feature = "common")]
pub mod common;

/// Re-export core metrics crate.
pub use metrics;
