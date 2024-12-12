//! Commonly used types and traits in Reth.
//!
//! This crate contains various primitive traits used across reth's components.
//! It provides the [`Block`] trait which is used to represent a block and all its components.
//! A [`Block`] is composed of a [`Header`] and a [`BlockBody`]. In ethereum (and optimism), a block
//! body consists of a list of transactions, a list of uncle headers, and a list of withdrawals. For
//! optimism, uncle headers and withdrawals are always empty lists.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `op`: Implements the traits for various [op-alloy](https://github.com/alloy-rs/op-alloy)
//!   types.
//! - `reth-codec`: Enables db codec support for reth types including zstd compression for certain
//!   types.
//! - `serde`: Adds serde support for all types.
//! - `secp256k1`: Adds secp256k1 support for transaction signing/recovery. (By default the no-std
//!   friendly `k256` is used)

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

#[macro_use]
extern crate alloc;

/// Common constants.
pub mod constants;
pub use constants::gas_units::{format_gas, format_gas_throughput};

/// Minimal account
pub mod account;
pub use account::{Account, Bytecode};

pub mod receipt;
pub use receipt::{FullReceipt, Receipt};

pub mod transaction;
pub use transaction::{
    execute::FillTxEnv,
    signed::{FullSignedTx, SignedTransaction},
    FullTransaction, Transaction,
};

pub mod block;
pub use block::{
    body::{BlockBody, FullBlockBody},
    header::{BlockHeader, FullBlockHeader},
    Block, FullBlock,
};

mod encoded;
mod withdrawal;
pub use encoded::WithEncoded;

pub mod crypto;

mod error;
pub use error::{GotExpected, GotExpectedBoxed};

mod log;
pub use alloy_primitives::{logs_bloom, Log, LogData};

mod storage;
pub use storage::StorageEntry;

/// Common header types
pub mod header;
#[cfg(any(test, feature = "arbitrary", feature = "test-utils"))]
pub use header::test_utils;
pub use header::{Header, HeaderError, SealedHeader};

/// Bincode-compatible serde implementations for common abstracted types in Reth.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// Reth types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat;

/// Heuristic size trait
pub mod size;
pub use size::InMemorySize;

/// Node traits
pub mod node;
pub use node::{BodyTy, FullNodePrimitives, HeaderTy, NodePrimitives, ReceiptTy};

/// Helper trait that requires de-/serialize implementation since `serde` feature is enabled.
#[cfg(feature = "serde")]
pub trait MaybeSerde: serde::Serialize + for<'de> serde::Deserialize<'de> {}
/// Noop. Helper trait that would require de-/serialize implementation if `serde` feature were
/// enabled.
#[cfg(not(feature = "serde"))]
pub trait MaybeSerde {}

#[cfg(feature = "serde")]
impl<T> MaybeSerde for T where T: serde::Serialize + for<'de> serde::Deserialize<'de> {}
#[cfg(not(feature = "serde"))]
impl<T> MaybeSerde for T {}

/// Helper trait that requires database encoding implementation since `reth-codec` feature is
/// enabled.
#[cfg(feature = "reth-codec")]
pub trait MaybeCompact: reth_codecs::Compact {}
/// Noop. Helper trait that would require database encoding implementation if `reth-codec` feature
/// were enabled.
#[cfg(not(feature = "reth-codec"))]
pub trait MaybeCompact {}

#[cfg(feature = "reth-codec")]
impl<T> MaybeCompact for T where T: reth_codecs::Compact {}
#[cfg(not(feature = "reth-codec"))]
impl<T> MaybeCompact for T {}

/// Helper trait that requires serde bincode compatibility implementation.
#[cfg(feature = "serde-bincode-compat")]
pub trait MaybeSerdeBincodeCompat: crate::serde_bincode_compat::SerdeBincodeCompat {}
/// Noop. Helper trait that would require serde bincode compatibility implementation if
/// `serde-bincode-compat` feature were enabled.
#[cfg(not(feature = "serde-bincode-compat"))]
pub trait MaybeSerdeBincodeCompat {}

#[cfg(feature = "serde-bincode-compat")]
impl<T> MaybeSerdeBincodeCompat for T where T: crate::serde_bincode_compat::SerdeBincodeCompat {}
#[cfg(not(feature = "serde-bincode-compat"))]
impl<T> MaybeSerdeBincodeCompat for T {}
