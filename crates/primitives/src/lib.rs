//! Commonly used types in Reth.
//!
//! This crate contains Ethereum primitive types and helper functions.
//!
//! ## Feature Flags
//!
//! - `alloy-compat`: Adds compatibility conversions for certain alloy types.
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `test-utils`: Export utilities for testing
//! - `reth-codec`: Enables db codec support for reth types including zstd compression for certain
//!   types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "alloy-compat")]
mod alloy_compat;
mod block;
#[cfg(feature = "reth-codec")]
mod compression;
pub mod proofs;
mod receipt;
pub use reth_static_file_types as static_file;
pub mod transaction;
#[cfg(any(test, feature = "arbitrary"))]
pub use block::{generate_valid_header, valid_header_strategy};
pub use block::{Block, BlockBody, BlockWithSenders, SealedBlock, SealedBlockWithSenders};
#[cfg(feature = "reth-codec")]
pub use compression::*;
pub use receipt::{
    gas_spent_by_transactions, Receipt, ReceiptWithBloom, ReceiptWithBloomRef, Receipts,
};
pub use reth_primitives_traits::{
    logs_bloom, Account, Bytecode, GotExpected, GotExpectedBoxed, Header, HeaderError, Log,
    LogData, NodePrimitives, SealedHeader, StorageEntry,
};
pub use static_file::StaticFileSegment;

pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer_unchecked, sign_message},
    BlobTransaction, InvalidTransactionError, PooledTransactionsElement,
    PooledTransactionsElementEcRecovered, Transaction, TransactionMeta, TransactionSigned,
    TransactionSignedEcRecovered, TransactionSignedNoHash, TxHashOrNumber, TxType,
};

// Re-exports
pub use reth_ethereum_forks::*;

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;

#[cfg(feature = "c-kzg")]
pub use c_kzg as kzg;

/// Bincode-compatible serde implementations for commonly used types in Reth.
///
/// `bincode` crate doesn't work with optionally serializable serde fields, but some of the
/// Reth types require optional serialization for RPC compatibility. This module makes so that
/// all fields are serialized.
///
/// Read more: <https://github.com/bincode-org/bincode/issues/326>
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::{
        block::serde_bincode_compat::*,
        transaction::{serde_bincode_compat as transaction, serde_bincode_compat::*},
    };
}

/// Temp helper struct for integrating [`NodePrimitives`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EthPrimitives;

#[cfg(feature = "reth-codec")]
impl reth_primitives_traits::FullNodePrimitives for EthPrimitives {
    type Block = crate::Block;
    type SignedTx = crate::TransactionSigned;
    type TxType = crate::TxType;
    type Receipt = crate::Receipt;
}

#[cfg(not(feature = "reth-codec"))]
impl NodePrimitives for EthPrimitives {
    type Block = crate::Block;
    type SignedTx = crate::TransactionSigned;
    type TxType = crate::TxType;
    type Receipt = crate::Receipt;
}
