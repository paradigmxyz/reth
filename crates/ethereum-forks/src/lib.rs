//! Ethereum fork types used in reth.
//!
//! This crate contains Ethereum fork types and helper functions.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod forkid;
mod hardfork;
mod head;

pub use forkid::{ForkFilter, ForkFilterKey, ForkHash, ForkId, ForkTransition, ValidationError};
pub use hardfork::Hardfork;
pub use head::Head;

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;
