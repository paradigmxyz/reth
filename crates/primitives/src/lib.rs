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
pub mod constants;
pub mod proofs;
mod receipt;
pub use reth_static_file_types as static_file;
pub mod transaction;
#[cfg(any(test, feature = "arbitrary"))]
pub use block::{generate_valid_header, valid_header_strategy};
pub use block::{
    Block, BlockBody, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, BlockWithSenders,
    ForkBlock, RpcBlockHash, SealedBlock, SealedBlockWithSenders,
};
#[cfg(feature = "reth-codec")]
pub use compression::*;
pub use constants::{
    DEV_GENESIS_HASH, EMPTY_OMMER_ROOT_HASH, HOLESKY_GENESIS_HASH, KECCAK_EMPTY,
    MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH,
};
pub use receipt::{
    gas_spent_by_transactions, Receipt, ReceiptWithBloom, ReceiptWithBloomRef, Receipts,
};
pub use reth_primitives_traits::{
    logs_bloom, Account, Bytecode, GotExpected, GotExpectedBoxed, Header, HeaderError, Log,
    LogData, Request, Requests, SealedHeader, StorageEntry, Withdrawal, Withdrawals,
};
pub use static_file::StaticFileSegment;

pub use transaction::{
    BlobTransaction, BlobTransactionSidecar, PooledTransactionsElement,
    PooledTransactionsElementEcRecovered,
};

#[cfg(feature = "c-kzg")]
pub use transaction::BlobTransactionValidationError;

pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer_unchecked, sign_message},
    InvalidTransactionError, Signature, Transaction, TransactionMeta, TransactionSigned,
    TransactionSignedEcRecovered, TransactionSignedNoHash, TxHashOrNumber, TxType,
    EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};

// Re-exports
pub use reth_ethereum_forks::*;
pub use revm_primitives::{self, JumpTable};

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
