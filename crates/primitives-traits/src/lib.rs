//! Common abstracted types in Reth.

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
    FullTransaction, Transaction, TransactionExt,
};

mod integer_list;
pub use integer_list::{IntegerList, IntegerListError};

pub mod block;
pub use block::{
    body::{BlockBody, FullBlockBody},
    header::{BlockHeader, FullBlockHeader},
    Block, FullBlock,
};

mod withdrawal;

mod error;
pub use error::{GotExpected, GotExpectedBoxed};

mod log;
pub use alloy_primitives::{logs_bloom, Log, LogData};

mod storage;
pub use storage::StorageEntry;

/// Transaction types
pub mod tx_type;
pub use tx_type::{FullTxType, TxType};

/// Common header types
pub mod header;
#[cfg(any(test, feature = "arbitrary", feature = "test-utils"))]
pub use header::test_utils;
pub use header::{BlockWithParent, Header, HeaderError, SealedHeader};

/// Bincode-compatible serde implementations for common abstracted types in Reth.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// Reth types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::header::{serde_bincode_compat as header, serde_bincode_compat::*};
}

/// Heuristic size trait
pub mod size;
pub use size::InMemorySize;

/// Node traits
pub mod node;
pub use node::{FullNodePrimitives, NodePrimitives, ReceiptTy};

/// Helper trait that requires arbitrary implementation if the feature is enabled.
#[cfg(any(feature = "test-utils", feature = "arbitrary"))]
pub trait MaybeArbitrary: for<'a> arbitrary::Arbitrary<'a> {}
/// Helper trait that requires arbitrary implementation if the feature is enabled.
#[cfg(not(any(feature = "test-utils", feature = "arbitrary")))]
pub trait MaybeArbitrary {}

#[cfg(any(feature = "test-utils", feature = "arbitrary"))]
impl<T> MaybeArbitrary for T where T: for<'a> arbitrary::Arbitrary<'a> {}
#[cfg(not(any(feature = "test-utils", feature = "arbitrary")))]
impl<T> MaybeArbitrary for T {}

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
