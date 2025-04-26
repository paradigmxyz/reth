//! Optimism P2P network data primitives.

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
