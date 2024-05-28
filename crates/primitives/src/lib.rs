//! Commonly used types in reth.
//!
//! This crate contains Ethereum primitive types and helper functions.
//!
//! ## Feature Flags
//!
//! - `alloy-compat`: Adds compatibility conversions for certain alloy types.
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod account;
#[cfg(feature = "alloy-compat")]
mod alloy_compat;
pub mod basefee;
mod block;
mod chain;
#[cfg(feature = "zstd-codec")]
mod compression;
pub mod constants;
pub mod eip4844;
mod error;
mod exex;
pub mod genesis;
mod header;
mod integer_list;
mod log;
mod net;
pub mod proofs;
mod prune;
mod receipt;
mod request;
/// Helpers for working with revm
pub mod revm;
pub mod stage;
pub use reth_static_file_types as static_file;
mod storage;
pub mod transaction;
pub mod trie;
mod withdrawal;
pub use account::{Account, Bytecode};
#[cfg(any(test, feature = "arbitrary"))]
pub use block::{generate_valid_header, valid_header_strategy};
pub use block::{
    Block, BlockBody, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, BlockWithSenders,
    ForkBlock, RpcBlockHash, SealedBlock, SealedBlockWithSenders,
};
pub use chain::{
    AllGenesisFormats, BaseFeeParams, BaseFeeParamsKind, Chain, ChainInfo, ChainKind, ChainSpec,
    ChainSpecBuilder, DisplayHardforks, ForkBaseFeeParams, ForkCondition, NamedChain, DEV, GOERLI,
    HOLESKY, MAINNET, SEPOLIA,
};
#[cfg(feature = "zstd-codec")]
pub use compression::*;
pub use constants::{
    DEV_GENESIS_HASH, EMPTY_OMMER_ROOT_HASH, GOERLI_GENESIS_HASH, HOLESKY_GENESIS_HASH,
    KECCAK_EMPTY, MAINNET_GENESIS_HASH, SEPOLIA_GENESIS_HASH,
};
pub use error::{GotExpected, GotExpectedBoxed};
pub use exex::FinishedExExHeight;
pub use genesis::{ChainConfig, Genesis, GenesisAccount};
pub use header::{Header, HeaderValidationError, HeadersDirection, SealedHeader};
pub use integer_list::IntegerList;
pub use log::{logs_bloom, Log};
pub use net::{
    goerli_nodes, holesky_nodes, mainnet_nodes, parse_nodes, sepolia_nodes, NodeRecord,
    NodeRecordParseError, GOERLI_BOOTNODES, HOLESKY_BOOTNODES, MAINNET_BOOTNODES,
    SEPOLIA_BOOTNODES,
};
pub use prune::{
    PruneCheckpoint, PruneInterruptReason, PruneLimiter, PruneMode, PruneModes, PruneProgress,
    PrunePurpose, PruneSegment, PruneSegmentError, ReceiptsLogPruneConfig,
    MINIMUM_PRUNING_DISTANCE,
};
pub use receipt::{
    gas_spent_by_transactions, Receipt, ReceiptWithBloom, ReceiptWithBloomRef, Receipts,
};
pub use request::Requests;
pub use static_file::StaticFileSegment;
pub use storage::StorageEntry;

pub use transaction::{
    BlobTransaction, BlobTransactionSidecar, FromRecoveredPooledTransaction,
    PooledTransactionsElement, PooledTransactionsElementEcRecovered,
};

#[cfg(feature = "c-kzg")]
pub use transaction::BlobTransactionValidationError;

pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer_unchecked, sign_message},
    AccessList, AccessListItem, IntoRecoveredTransaction, InvalidTransactionError, Signature,
    Transaction, TransactionMeta, TransactionSigned, TransactionSignedEcRecovered,
    TransactionSignedNoHash, TryFromRecoveredTransaction, TxEip1559, TxEip2930, TxEip4844,
    TxHashOrNumber, TxLegacy, TxType, EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};

pub use withdrawal::{Withdrawal, Withdrawals};

// Re-exports
pub use self::ruint::UintTryTo;
pub use alloy_consensus::Request;
pub use alloy_primitives::{
    self, address, b256, bloom, bytes,
    bytes::{Buf, BufMut, BytesMut},
    eip191_hash_message, hex, hex_literal, keccak256, ruint,
    utils::format_ether,
    Address, BlockHash, BlockNumber, Bloom, BloomInput, Bytes, ChainId, Selector, StorageKey,
    StorageValue, TxHash, TxIndex, TxKind, TxNumber, B128, B256, B512, B64, U128, U256, U64, U8,
};
pub use reth_ethereum_forks::*;
pub use revm_primitives::{self, JumpTable};

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
        chain::{BASE_MAINNET, BASE_SEPOLIA, OP_MAINNET, OP_SEPOLIA},
        net::{
            base_nodes, base_testnet_nodes, op_nodes, op_testnet_nodes, OP_BOOTNODES,
            OP_TESTNET_BOOTNODES,
        },
        transaction::{TxDeposit, DEPOSIT_TX_TYPE_ID},
    };
}

#[cfg(feature = "optimism")]
pub use optimism::*;
