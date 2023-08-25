#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Commonly used types in reth.
//!
//! This crate contains Ethereum primitive types and helper functions.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `test-utils`: Export utilities for testing
pub mod abi;
mod account;
pub mod basefee;
mod bits;
mod block;
pub mod bloom;
mod chain;
mod compression;
pub mod constants;
pub mod contract;
pub mod eip4844;
mod forkid;
pub mod fs;
mod genesis;
mod hardfork;
mod header;
mod hex_bytes;
mod integer_list;
pub mod listener;
mod log;
mod net;
mod peer;
mod prune;
mod receipt;
pub mod stage;
mod storage;
mod transaction;
pub mod trie;
mod withdrawal;

/// Helper function for calculating Merkle proofs and hashes
pub mod proofs;

pub use account::{Account, Bytecode};
pub use bits::H512;
pub use block::{
    Block, BlockBody, BlockBodyRoots, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag,
    BlockWithSenders, ForkBlock, SealedBlock, SealedBlockWithSenders,
};
pub use bloom::Bloom;
pub use chain::{
    AllGenesisFormats, BaseFeeParams, Chain, ChainInfo, ChainSpec, ChainSpecBuilder,
    DisplayHardforks, ForkCondition, ForkTimestamps, DEV, GOERLI, MAINNET, SEPOLIA,
};
pub use compression::*;
pub use constants::{
    DEV_GENESIS, EMPTY_OMMER_ROOT, GOERLI_GENESIS, KECCAK_EMPTY, MAINNET_GENESIS, SEPOLIA_GENESIS,
};
pub use eip4844::{calculate_excess_blob_gas, kzg_to_versioned_hash};
pub use forkid::{ForkFilter, ForkHash, ForkId, ForkTransition, ValidationError};
pub use genesis::{Genesis, GenesisAccount};
pub use hardfork::Hardfork;
pub use header::{Head, Header, HeadersDirection, SealedHeader};
pub use hex_bytes::Bytes;
pub use integer_list::IntegerList;
pub use log::Log;
pub use net::{
    goerli_nodes, mainnet_nodes, sepolia_nodes, NodeRecord, GOERLI_BOOTNODES, MAINNET_BOOTNODES,
    SEPOLIA_BOOTNODES,
};
pub use peer::{PeerId, WithPeerId};
pub use prune::{
    PruneBatchSizes, PruneCheckpoint, PruneMode, PruneModes, PrunePart, PrunePartError,
    ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
pub use receipt::{Receipt, ReceiptWithBloom, ReceiptWithBloomRef};
pub use revm_primitives::JumpMap;
pub use serde_helper::JsonU256;
pub use storage::StorageEntry;
pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer, sign_message},
    AccessList, AccessListItem, AccessListWithGasUsed, BlobTransaction, BlobTransactionSidecar,
    BlobTransactionValidationError, FromRecoveredPooledTransaction, FromRecoveredTransaction,
    IntoRecoveredTransaction, InvalidTransactionError, PooledTransactionsElement,
    PooledTransactionsElementEcRecovered, Signature, Transaction, TransactionKind, TransactionMeta,
    TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash, TxEip1559, TxEip2930,
    TxEip4844, TxLegacy, TxType, EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};
pub use withdrawal::Withdrawal;

/// A block hash.
pub type BlockHash = H256;
/// A block number.
pub type BlockNumber = u64;
/// An Ethereum address.
pub type Address = H160;
/// A transaction hash is a kecack hash of an RLP encoded signed transaction.
pub type TxHash = H256;
/// The sequence number of all existing transactions.
pub type TxNumber = u64;
/// The index of transaction in a block.
pub type TxIndex = u64;
/// Chain identifier type (introduced in EIP-155).
pub type ChainId = u64;
/// An account storage key.
pub type StorageKey = H256;
/// An account storage value.
pub type StorageValue = U256;
/// Solidity contract functions are addressed using the first four byte of the Keccak-256 hash of
/// their signature
pub type Selector = [u8; 4];

pub use ethers_core::{
    types::{BigEndianHash, H128, H64, U64},
    utils as rpc_utils,
};
pub use revm_primitives::{B160 as H160, B256 as H256, U256};
pub use ruint::{
    aliases::{U128, U8},
    UintTryTo,
};

#[doc(hidden)]
mod __reexport {
    pub use bytes;
    pub use hex;
    pub use hex_literal;
    pub use tiny_keccak;
}

// Useful reexports
pub use __reexport::*;

/// Various utilities
pub mod utils {
    pub use ethers_core::types::serde_helpers;
}

/// EIP-4844 + KZG helpers
pub mod kzg {
    pub use c_kzg::*;
}

/// Helpers for working with serde
pub mod serde_helper;

/// Returns the keccak256 hash for the given data.
#[inline]
pub fn keccak256(data: impl AsRef<[u8]>) -> H256 {
    use tiny_keccak::{Hasher, Keccak};

    let mut buf = [0u8; 32];
    let mut hasher = Keccak::v256();
    hasher.update(data.as_ref());
    hasher.finalize(&mut buf);
    buf.into()
}

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;
