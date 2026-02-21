//! Commonly used types in Reth.
//!
//! ## Deprecation Notice
//!
//! This crate is deprecated and will be removed in a future release.
//! Use [`reth-ethereum-primitives`](https://crates.io/crates/reth-ethereum-primitives) and
//! [`reth-primitives-traits`](https://crates.io/crates/reth-primitives-traits) instead.

#![cfg_attr(
    not(feature = "__internal"),
    deprecated(
        note = "the `reth-primitives` crate is deprecated, use `reth-ethereum-primitives` and `reth-primitives-traits` instead."
    )
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(deprecated)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

// These are used as optional dependencies solely for feature forwarding.
#[cfg(feature = "alloy-eips")]
use alloy_eips as _;
#[cfg(feature = "alloy-genesis")]
use alloy_genesis as _;
#[cfg(feature = "alloy-primitives")]
use alloy_primitives as _;
#[cfg(feature = "alloy-rlp")]
use alloy_rlp as _;
use once_cell as _;
#[cfg(feature = "reth-codecs")]
use reth_codecs as _;

// --- block types ---

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type Block<T = TransactionSigned, H = Header> = alloy_consensus::Block<T, H>;

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
pub type BlockBody<T = TransactionSigned, H = Header> = alloy_consensus::BlockBody<T, H>;

/// Ethereum sealed block type
pub type SealedBlock<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Helper type for constructing the block
#[deprecated(note = "Use `SealedBlock` instead")]
pub type SealedBlockFor<B = Block> = reth_primitives_traits::block::SealedBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type BlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;

/// Ethereum recovered block
#[deprecated(note = "Use `RecoveredBlock` instead")]
pub type SealedBlockWithSenders<B = Block> = reth_primitives_traits::block::RecoveredBlock<B>;

#[cfg(any(test, feature = "arbitrary"))]
pub use reth_primitives_traits::test_utils::{generate_valid_header, valid_header_strategy};

// --- receipt types ---

pub use reth_ethereum_primitives::Receipt;
pub use reth_primitives_traits::receipt::gas_spent_by_transactions;

// --- transaction types ---

/// Transaction types re-exported for backward compatibility.
pub mod transaction {
    pub use alloy_consensus::{transaction::PooledTransaction, TxType};
    pub use reth_ethereum_primitives::{Transaction, TransactionSigned};
    pub use reth_primitives_traits::{
        crypto::secp256k1::{recover_signer, recover_signer_unchecked},
        transaction::{
            error::{
                InvalidTransactionError, TransactionConversionError,
                TryFromRecoveredTransactionError,
            },
            signed::SignedTransaction,
        },
        FillTxEnv, WithEncoded,
    };

    /// Utility functions for signatures.
    pub mod util {
        pub use reth_primitives_traits::crypto::*;
    }

    /// Signed transaction.
    pub mod signature {
        pub use reth_primitives_traits::crypto::secp256k1::{
            recover_signer, recover_signer_unchecked,
        };
    }

    use super::Recovered;
    use alloy_consensus::transaction::PooledTransaction as PooledTx;

    /// A signed pooled transaction with recovered signer.
    #[deprecated(note = "use `Recovered` instead")]
    pub type PooledTransactionsElementEcRecovered<T = PooledTx> = Recovered<T>;

    /// Type alias kept for backward compatibility.
    #[deprecated(note = "Use `Recovered` instead")]
    pub type TransactionSignedEcRecovered<T = TransactionSigned> = Recovered<T>;
}

pub use transaction::{
    util::secp256k1::{public_key_to_address, recover_signer_unchecked, sign_message},
    InvalidTransactionError, Transaction, TransactionSigned, TxType,
};
#[expect(deprecated)]
pub use transaction::{PooledTransactionsElementEcRecovered, TransactionSignedEcRecovered};

// --- common re-exports ---

pub use reth_static_file_types as static_file;
pub use static_file::StaticFileSegment;

pub use reth_primitives_traits::{
    logs_bloom, Account, BlockTy, BodyTy, Bytecode, GotExpected, GotExpectedBoxed, Header,
    HeaderTy, Log, LogData, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader, StorageEntry,
    TxTy,
};

pub use alloy_consensus::{
    transaction::{PooledTransaction, Recovered, TransactionMeta},
    ReceiptWithBloom,
};

/// Recovered transaction
#[deprecated(note = "use `Recovered` instead")]
pub type RecoveredTx<T> = Recovered<T>;

pub use reth_ethereum_forks::*;

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
    pub use reth_primitives_traits::serde_bincode_compat::*;
}

pub use reth_ethereum_primitives::EthPrimitives;
