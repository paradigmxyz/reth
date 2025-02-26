//! Ethereum fork types used in reth.
//!
//! This crate contains Ethereum fork types and helper functions.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `arbitrary` support for primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// Re-exported EIP-2124 forkid types.
pub use alloy_eip2124::*;

mod display;
mod forkcondition;
mod hardfork;
mod hardforks;

pub use hardfork::{EthereumHardfork, Hardfork, DEV_HARDFORKS};

pub use display::DisplayHardforks;
pub use forkcondition::ForkCondition;
pub use hardforks::*;

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;
