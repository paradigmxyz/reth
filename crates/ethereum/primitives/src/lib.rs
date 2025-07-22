//! Standalone crate for ethereum-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod receipt;
pub use receipt::*;

/// Kept for consistency tests
#[cfg(test)]
mod transaction;

pub use alloy_consensus::{transaction::PooledTransaction, TxType};
use alloy_consensus::{TxEip4844, TxEip4844WithSidecar};
use alloy_eips::eip7594::BlobTransactionSidecarVariant;

/// Typed Transaction type without a signature
pub type Transaction = alloy_consensus::EthereumTypedTransaction<TxEip4844>;

/// Signed transaction.
pub type TransactionSigned = alloy_consensus::EthereumTxEnvelope<TxEip4844>;

/// A type alias for [`PooledTransaction`] that's also generic over blob sidecar.
pub type PooledTransactionVariant =
    alloy_consensus::EthereumTxEnvelope<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>;

/// Bincode-compatible serde implementations.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    pub use super::receipt::serde_bincode_compat::*;
    pub use alloy_consensus::serde_bincode_compat::transaction::*;
}

/// Type alias for the ethereum block
pub type Block = alloy_consensus::Block<TransactionSigned>;

/// Type alias for the ethereum blockbody
pub type BlockBody = alloy_consensus::BlockBody<TransactionSigned>;

/// Helper struct that specifies the ethereum
/// [`NodePrimitives`](reth_primitives_traits::NodePrimitives) types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct EthPrimitives;

impl reth_primitives_traits::NodePrimitives for EthPrimitives {
    type Block = crate::Block;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = crate::BlockBody;
    type SignedTx = crate::TransactionSigned;
    type Receipt = crate::Receipt;
}
