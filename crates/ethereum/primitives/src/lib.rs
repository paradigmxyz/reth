//! L1 Ethereum data primitives.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

use alloy_consensus::Header;
use reth_primitives_traits::NodePrimitives;
use reth_primitives::{Block, BlockBody, Receipt, TxType, TransactionSigned};

/// Ethereum primitive types.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EthPrimitives;

impl NodePrimitives for EthPrimitives {
    type Block = Block;
    type BlockHeader = Header;
    type BlockBody = BlockBody;
    type SignedTx = TransactionSigned;
    type TxType = TxType;
    type Receipt = Receipt;
}
