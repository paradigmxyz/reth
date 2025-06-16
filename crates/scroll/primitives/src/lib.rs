//! Commonly used types in Scroll.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/scroll-tech/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

use once_cell as _;

#[cfg(not(feature = "std"))]
extern crate alloc as std;

pub mod transaction;
pub use transaction::{tx_type::ScrollTxType, ScrollTransactionSigned};

use reth_primitives_traits::Block;

mod receipt;
pub use receipt::ScrollReceipt;

/// Scroll-specific block type.
pub type ScrollBlock = alloy_consensus::Block<ScrollTransactionSigned>;

/// Scroll-specific block body type.
pub type ScrollBlockBody = <ScrollBlock as Block>::Body;

/// Primitive types for Scroll Node.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScrollPrimitives;

#[cfg(feature = "serde-bincode-compat")]
impl reth_primitives_traits::NodePrimitives for ScrollPrimitives {
    type Block = ScrollBlock;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = ScrollBlockBody;
    type SignedTx = ScrollTransactionSigned;
    type Receipt = ScrollReceipt;
}
