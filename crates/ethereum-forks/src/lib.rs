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
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

mod display;
mod forkcondition;
mod forkid;
mod hardfork;
mod hardforks;
mod head;

pub use forkid::{
    EnrForkIdEntry, ForkFilter, ForkFilterKey, ForkHash, ForkId, ForkTransition, ValidationError,
};
pub use hardfork::EthereumHardfork;
pub use head::Head;

pub use chains::ethereum::*;
pub use display::DisplayHardforks;
pub use forkcondition::ForkCondition;
pub use hardfork::Hardfork;
pub use hardforks::*;

#[cfg(feature = "optimism")]
pub use chains::optimism::*;
#[cfg(feature = "optimism")]
pub use hardfork::optimism::*;

/// Chains hardforks
pub mod chains;

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;
