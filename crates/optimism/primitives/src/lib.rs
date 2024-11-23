//! Standalone crate for Optimism-specific Reth primitive types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

pub mod bedrock;
pub mod tx_type;

pub use tx_type::OpTxType;

use alloy_consensus::Header;
use reth_node_types::NodePrimitives;
use reth_primitives::{Block, BlockBody, Receipt, TransactionSigned};

/// Optimism primitive types.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpPrimitives;

impl NodePrimitives for OpPrimitives {
    type Block = Block;
    type BlockHeader = Header;
    type BlockBody = BlockBody;
    type SignedTx = TransactionSigned;
    type TxType = OpTxType;
    type Receipt = Receipt;
}
