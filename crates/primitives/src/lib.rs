//! Commonly used types in reth.
//!
//! This crate contains Ethereum primitive types and helper functions.
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
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod account;
pub mod basefee;
mod block;
mod chain;
mod compression;
pub mod constants;
pub mod eip4844;
mod error;
pub mod fs;
mod genesis;
mod header;
mod integer_list;
mod log;
mod net;
mod peer;
pub mod proofs;
mod prune;
mod receipt;
/// Helpers for working with revm
pub mod revm;
pub mod serde_helper;
pub mod snapshot;
pub mod stage;
mod storage;
/// Helpers for working with transactions
pub mod transaction;
pub mod trie;
mod withdrawal;

pub use account::{Account, Bytecode};
pub use block::{
    Block, BlockBody, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, BlockWithSenders,
    ForkBlock, RpcBlockHash, SealedBlock, SealedBlockWithSenders,
};
pub use chain::{
    AllGenesisFormats, BaseFeeParams, BaseFeeParamsKind, Chain, ChainInfo, ChainSpec,
    ChainSpecBuilder, DisplayHardforks, ForkBaseFeeParams, ForkCondition, ForkTimestamps,
    NamedChain, DEV, GOERLI, HOLESKY, MAINNET, SEPOLIA,
};
pub use compression::*;
pub use constants::{
    DEV_GENESIS_HASH, EMPTY_OMMER_ROOT_HASH, GOERLI_GENESIS_HASH, HOLESKY_GENESIS_HASH,
    KECCAK_EMPTY, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH,
};
pub use error::{GotExpected, GotExpectedBoxed};
pub use genesis::{ChainConfig, Genesis, GenesisAccount};
pub use header::{Header, HeadersDirection, SealedHeader};
pub use integer_list::IntegerList;
pub use log::{logs_bloom, Log};
pub use net::{
    goerli_nodes, holesky_nodes, mainnet_nodes, parse_nodes, sepolia_nodes, NodeRecord,
    GOERLI_BOOTNODES, HOLESKY_BOOTNODES, MAINNET_BOOTNODES, SEPOLIA_BOOTNODES,
};
pub use peer::{PeerId, WithPeerId};
pub use prune::{
    PruneCheckpoint, PruneMode, PruneModes, PruneProgress, PruneSegment, PruneSegmentError,
    ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
pub use receipt::{Receipt, ReceiptWithBloom, ReceiptWithBloomRef, Receipts};
pub use serde_helper::JsonU256;
pub use snapshot::SnapshotSegment;
pub use storage::StorageEntry;

#[cfg(feature = "c-kzg")]
pub use transaction::{
    BlobTransaction, BlobTransactionSidecar, BlobTransactionValidationError,
    FromRecoveredPooledTransaction, PooledTransactionsElement,
    PooledTransactionsElementEcRecovered,
};

pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer_unchecked, sign_message},
    AccessList, AccessListItem, FromRecoveredTransaction, IntoRecoveredTransaction,
    InvalidTransactionError, Signature, Transaction, TransactionKind, TransactionMeta,
    TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash, TxEip1559, TxEip2930,
    TxEip4844, TxHashOrNumber, TxLegacy, TxType, TxValue, EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID,
    EIP4844_TX_TYPE_ID, LEGACY_TX_TYPE_ID,
};
pub use withdrawal::Withdrawal;

// Re-exports
pub use self::ruint::UintTryTo;
pub use alloy_primitives::{
    self, address, b256, bloom, bytes,
    bytes::{Buf, BufMut, BytesMut},
    eip191_hash_message, hex, hex_literal, keccak256, ruint, Address, BlockHash, BlockNumber,
    Bloom, BloomInput, Bytes, ChainId, Selector, StorageKey, StorageValue, TxHash, TxIndex,
    TxNumber, B128, B256, B512, B64, U128, U256, U64, U8,
};
pub use reth_ethereum_forks::*;
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

/// Optimism specific re-exports
#[cfg(feature = "optimism")]
mod optimism {
    pub use crate::{
        chain::{BASE_GOERLI, BASE_MAINNET, BASE_SEPOLIA, OP_GOERLI},
        transaction::{TxDeposit, DEPOSIT_TX_TYPE_ID},
    };
}

#[cfg(feature = "optimism")]
pub use optimism::*;
