//! Commonly used types and traits in Reth.
//!
//! ## Overview
//!
//! This crate defines various traits and types that form the foundation of the reth stack.
//! The top-level trait is [`Block`] which represents a block in the blockchain. A [`Block`] is
//! composed of a [`Header`] and a [`BlockBody`]. A [`BlockBody`] contains the transactions in the
//! block and additional data that is part of the block. In ethereum, this includes uncle headers
//! and withdrawals. For optimism, uncle headers and withdrawals are always empty lists.
//!
//! The most common types you'll use are:
//! - [`Block`] - A basic block with header and body
//! - [`SealedBlock`] - A block with its hash cached
//! - [`SealedHeader`] - A header with its hash cached
//! - [`RecoveredBlock`] - A sealed block with sender addresses recovered
//!
//! ## Feature Flags
//!
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for primitive types.
//! - `op`: Implements the traits for various [op-alloy](https://github.com/alloy-rs/op-alloy)
//!   types.
//! - `reth-codec`: Enables db codec support for reth types including zstd compression for certain
//!   types.
//! - `rpc-compat`: Adds RPC compatibility functions for the types in this crate, e.g. rpc type
//!   conversions.
//! - `serde`: Adds serde support for all types.
//! - `secp256k1`: Adds secp256k1 support for transaction signing/recovery. (By default the no-std
//!   friendly `k256` is used)
//! - `rayon`: Uses `rayon` for parallel transaction sender recovery in [`BlockBody`] by default.
//! - `serde-bincode-compat` provides helpers for dealing with the `bincode` crate.
//!
//! ### Sealing (Hashing)
//!
//! The block hash is derived from the [`Header`] and is used to uniquely identify the block. This
//! operation is referred to as sealing in the context of this crate. Sealing is an expensive
//! operation. This crate provides various wrapper types that cache the hash of the block to avoid
//! recomputing it: [`SealedHeader`] and [`SealedBlock`]. All sealed types can be downgraded to
//! their unsealed counterparts.
//!
//! ### Recovery
//!
//! The raw consensus transactions that make up a block don't include the sender's address. This
//! information is recovered from the transaction signature. This operation is referred to as
//! recovery in the context of this crate and is an expensive operation. The [`RecoveredBlock`]
//! represents a [`SealedBlock`] with the sender addresses recovered. A [`SealedBlock`] can be
//! upgraded to a [`RecoveredBlock`] by recovering the sender addresses:
//! [`SealedBlock::try_recover`]. A [`RecoveredBlock`] can be downgraded to a [`SealedBlock`] by
//! removing the sender addresses: [`RecoveredBlock::into_sealed_block`].
//!
//! #### Naming
//!
//! The types in this crate support multiple recovery functions, e.g.
//! [`SealedBlock::try_recover`] and [`SealedBlock::try_recover_unchecked`]. The `_unchecked` suffix indicates that this function recovers the signer _without ensuring that the signature has a low `s` value_, in other words this rule introduced in [EIP-2](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2.md) is ignored.
//! Hence this function is necessary when dealing with pre EIP-2 transactions on the ethereum
//! mainnet. Newer transactions must always be recovered with the regular `recover` functions, see
//! also [`recover_signer`](crypto::secp256k1::recover_signer).
//!
//! ## Error Handling
//!
//! Most operations that can fail return `Result` types:
//! - [`RecoveryError`](transaction::signed::RecoveryError) - Transaction signature recovery failed
//! - [`BlockRecoveryError`](block::error::BlockRecoveryError) - Block-level recovery failed
//! - [`GotExpected`] / [`GotExpectedBoxed`] - Generic error for mismatched values
//!
//! Recovery errors typically indicate invalid signatures or corrupted data. The block recovery
//! error preserves the original block for further inspection.
//!
//! ### Example
//!
//! ```rust
//! # use reth_primitives_traits::{SealedBlock, RecoveredBlock};
//! # use reth_primitives_traits::block::error::BlockRecoveryError;
//! # fn example<B: reth_primitives_traits::Block>(sealed_block: SealedBlock<B>) -> Result<(), BlockRecoveryError<SealedBlock<B>>>
//! # where B::Body: reth_primitives_traits::BlockBody<Transaction: reth_primitives_traits::SignedTransaction> {
//! // Attempt to recover senders from a sealed block
//! match sealed_block.try_recover() {
//!     Ok(recovered) => {
//!         // Successfully recovered all senders
//!         println!("Recovered {} senders", recovered.senders().len());
//!         Ok(())
//!     }
//!     Err(err) => {
//!         // Recovery failed - the block is returned in the error
//!         println!("Failed to recover senders for block");
//!         // You can still access the original block
//!         let block = err.into_inner();
//!         let hash = block.hash();
//!         Err(BlockRecoveryError::new(block))
//!     }
//! }
//! # }
//! ```
//!
//! ## Performance Considerations
//!
//! - **Hashing**: Block hashing is expensive. Use [`SealedBlock`] to cache hashes.
//! - **Recovery**: Sender recovery is CPU-intensive. Use [`RecoveredBlock`] to cache results.
//! - **Parallel Recovery**: Enable the `rayon` feature for parallel transaction recovery.
//!
//! ## Bincode serde compatibility
//!
//! The [bincode-crate](https://github.com/bincode-org/bincode) is often used by additional tools when sending data over the network.
//! `bincode` crate doesn't work well with optionally serializable serde fields, but some of the consensus types require optional serialization for RPC compatibility. Read more: <https://github.com/bincode-org/bincode/issues/326>
//!
//! As a workaround this crate introduces the `SerdeBincodeCompat` trait (available with the
//! `serde-bincode-compat` feature) used to provide a bincode compatible serde representation.

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
pub use alloy_consensus::{
    transaction::{Recovered, TransactionMeta},
    ReceiptWithBloom,
};

pub use transaction::{
    execute::FillTxEnv,
    signed::{FullSignedTx, SignedTransaction},
    FullTransaction, SignerRecoverable, Transaction,
};

pub mod block;
pub use block::{
    body::{BlockBody, FullBlockBody},
    header::{AlloyBlockHeader, BlockHeader, FullBlockHeader},
    recovered::IndexedTx,
    Block, FullBlock, RecoveredBlock, SealedBlock,
};

mod withdrawal;
pub use alloy_eips::eip2718::WithEncoded;

pub mod crypto;

mod error;
pub use error::{GotExpected, GotExpectedBoxed};

mod log;
pub use alloy_primitives::{logs_bloom, Log, LogData};

pub mod proofs;

mod storage;
pub use storage::StorageEntry;

pub mod sync;

mod extended;
pub use extended::Extended;
/// Common header types
pub mod header;
pub use header::{Header, SealedHeader, SealedHeaderFor};

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
pub use node::{BlockTy, BodyTy, FullNodePrimitives, HeaderTy, NodePrimitives, ReceiptTy, TxTy};

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

/// Utilities for testing.
#[cfg(any(test, feature = "arbitrary", feature = "test-utils"))]
pub mod test_utils {
    pub use crate::header::test_utils::{generate_valid_header, valid_header_strategy};
    #[cfg(any(test, feature = "test-utils"))]
    pub use crate::{block::TestBlock, header::test_utils::TestHeader};
}
