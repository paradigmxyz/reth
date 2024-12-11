//! Standalone crate for ethereum-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

mod receipt;
pub use receipt::*;

use reth_primitives_traits::NodePrimitives;
/// Primitive types for Ethereum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EthPrimitives;

impl NodePrimitives for EthPrimitives {
    type Block = reth_primitives::Block;
    type BlockBody = reth_primitives::BlockBody;
    type BlockHeader = reth_primitives::Header;
    type Receipt = Receipt;
    type SignedTx = reth_primitives::TransactionSigned;
    type TxType = reth_primitives::TxType;
}
