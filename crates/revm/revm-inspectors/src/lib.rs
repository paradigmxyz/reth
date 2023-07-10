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

//! revm [Inspector](revm::Inspector) implementations, such as call tracers
//!
//! ## Feature Flags
//!
//! - `js-tracer` (default): Enables a JavaScript tracer implementation. This pulls in extra
//!   dependencies (such as `boa`, `tokio` and `serde_json`).

/// An inspector implementation for an EIP2930 Accesslist
pub mod access_list;

/// An inspector stack abstracting the implementation details of
/// each inspector and allowing to hook on block/transaction execution,
/// used in the main RETH executor.
pub mod stack;

/// An inspector for recording traces
pub mod tracing;
