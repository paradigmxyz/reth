//! Ethereum fork types used in reth.
//!
//! This crate contains Ethereum fork types and helper functions.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::non_canonical_clone_impl)]

mod forkid;
mod hardfork;
mod head;

pub use forkid::{ForkFilter, ForkFilterKey, ForkHash, ForkId, ForkTransition, ValidationError};
pub use hardfork::Hardfork;
pub use head::Head;

// Re-exports
pub use self::ruint::UintTryTo;
pub use alloy_primitives::{
    self, address, b256, bloom, bytes, eip191_hash_message, hex, hex_literal, keccak256, ruint,
    Address, BlockHash, BlockNumber, Bloom, BloomInput, Bytes, ChainId, Selector, StorageKey,
    StorageValue, TxHash, TxIndex, TxNumber, B128, B256, B512, B64, U128, U256, U64, U8,
};
pub use revm_primitives::{self, JumpMap};

#[doc(hidden)]
#[deprecated = "use B64 instead"]
pub type H64 = B64;
#[doc(hidden)]
#[deprecated = "use B128 instead"]
pub type H128 = B128;
#[doc(hidden)]
#[deprecated = "use Address instead"]
pub type H160 = Address;
#[doc(hidden)]
#[deprecated = "use B256 instead"]
pub type H256 = B256;
#[doc(hidden)]
#[deprecated = "use B512 instead"]
pub type H512 = B512;

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;

#[cfg(feature = "c-kzg")]
pub use c_kzg as kzg;
