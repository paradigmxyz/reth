//! Optimism P2P network data primitives.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_consensus::Header;
use op_alloy_consensus::OpPooledTransaction;
use reth_eth_wire_types::NetworkPrimitives;
use reth_optimism_primitives::{OpBlock, OpBlockBody, OpReceipt, OpTransactionSigned};

/// Network primitive types used by Optimism networks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct OpNetworkPrimitives;

impl NetworkPrimitives for OpNetworkPrimitives {
    type BlockHeader = Header;
    type BlockBody = OpBlockBody;
    type Block = OpBlock;
    type BroadcastedTransaction = OpTransactionSigned;
    type PooledTransaction = OpPooledTransaction;
    type Receipt = OpReceipt;
}
