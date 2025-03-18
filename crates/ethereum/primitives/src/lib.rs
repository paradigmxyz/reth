//! Standalone crate for ethereum-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod receipt;
pub use receipt::*;

mod transaction;
pub use transaction::*;

#[cfg(feature = "alloy-compat")]
mod alloy_compat;

/// Bincode-compatible serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::{receipt::serde_bincode_compat::*, transaction::serde_bincode_compat::*};
}

/// Type alias for the ethereum block
pub type Block = alloy_consensus::Block<TransactionSigned>;

/// Type alias for the ethereum blockbody
pub type BlockBody = alloy_consensus::BlockBody<TransactionSigned>;

/// Helper struct that specifies the ethereum
/// [`NodePrimitives`](reth_primitives_traits::NodePrimitives) types.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct EthPrimitives;

impl reth_primitives_traits::NodePrimitives for EthPrimitives {
    type Block = crate::Block;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = crate::BlockBody;
    type SignedTx = crate::TransactionSigned;
    type Receipt = crate::Receipt;
}
