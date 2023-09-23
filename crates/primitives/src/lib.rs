//! Commonly used types in reth.
//!
//! This crate contains Ethereum primitive types and helper functions.
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `test-utils`: Export utilities for testing

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

pub mod abi;
mod account;
pub mod basefee;
mod block;
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
mod integer_list;
pub mod listener;
mod log;
mod net;
mod peer;
pub mod proofs;
mod prune;
mod receipt;
pub mod serde_helper;
pub mod stage;
mod storage;
mod transaction;
pub mod trie;
mod withdrawal;

pub use account::{Account, Bytecode};
pub use block::{
    Block, BlockBody, BlockBodyRoots, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag,
    BlockWithSenders, ForkBlock, SealedBlock, SealedBlockWithSenders,
};
pub use bytes::{Buf, BufMut, BytesMut};
pub use chain::{
    AllGenesisFormats, BaseFeeParams, Chain, ChainInfo, ChainSpec, ChainSpecBuilder,
    DisplayHardforks, ForkCondition, ForkTimestamps, DEV, GOERLI, HOLESKY, MAINNET, SEPOLIA,
};
pub use compression::*;
pub use constants::{
    DEV_GENESIS, EMPTY_OMMER_ROOT, GOERLI_GENESIS, HOLESKY_GENESIS, KECCAK_EMPTY, MAINNET_GENESIS,
    SEPOLIA_GENESIS,
};
pub use eip4844::{calculate_excess_blob_gas, kzg_to_versioned_hash};
pub use forkid::{ForkFilter, ForkHash, ForkId, ForkTransition, ValidationError};
pub use genesis::{Genesis, GenesisAccount};
pub use hardfork::Hardfork;
pub use header::{Head, Header, HeadersDirection, SealedHeader};
pub use integer_list::IntegerList;
pub use log::{logs_bloom, Log};
pub use net::{
    goerli_nodes, holesky_nodes, mainnet_nodes, sepolia_nodes, NodeRecord, GOERLI_BOOTNODES,
    HOLESKY_BOOTNODES, MAINNET_BOOTNODES, SEPOLIA_BOOTNODES,
};
pub use peer::{PeerId, WithPeerId};
pub use prune::{
    PruneBatchSizes, PruneCheckpoint, PruneMode, PruneModes, PrunePart, PrunePartError,
    ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
pub use receipt::{Receipt, ReceiptWithBloom, ReceiptWithBloomRef};
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

// Re-exports
pub use self::ruint::UintTryTo;
pub use alloy_primitives::{
    self, address, b256, bloom, bytes, hex, hex_literal, keccak256, ruint, Address, BlockHash,
    BlockNumber, Bloom, BloomInput, Bytes, ChainId, Selector, StorageKey, StorageValue, TxHash,
    TxIndex, TxNumber, B128 as H128, B256 as H256, B512 as H512, B64 as H64, U128, U256, U64, U8,
};
pub use revm_primitives::{self, JumpMap};

#[cfg(any(test, feature = "arbitrary"))]
pub use arbitrary;

/// Various utilities
pub mod utils {
    pub use ethers_core::types::serde_helpers;
}

/// EIP-4844 + KZG helpers
pub mod kzg {
    pub use c_kzg::*;
}
