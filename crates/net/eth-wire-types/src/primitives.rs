//! Abstraction over primitive types in network messages.

use reth_primitives_traits::node::NetworkPrimitives;
use std::fmt::Debug;

/// Primitive types used by Ethereum network.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EthNetworkPrimitives;

impl NetworkPrimitives for EthNetworkPrimitives {
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = reth_primitives::BlockBody;
    type Block = reth_primitives::Block;
    type BroadcastedTransaction = reth_primitives::TransactionSigned;
    type PooledTransaction = reth_primitives::PooledTransactionsElement;
    type Receipt = reth_primitives::Receipt;
}
