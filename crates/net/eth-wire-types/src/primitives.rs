//! Abstraction over primitive types in network messages.

use std::fmt::Debug;

use alloy_rlp::{Decodable, Encodable};
use reth_primitives_traits::{Block, BlockHeader};

/// Abstraction over primitive types which might appear in network messages. See
/// [`crate::EthMessage`] for more context.
pub trait NetworkPrimitives:
    Send + Sync + Unpin + Clone + Debug + PartialEq + Eq + 'static
{
    /// The block header type.
    type BlockHeader: BlockHeader
        + Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + Debug
        + PartialEq
        + Eq
        + 'static;
    /// The block body type.
    type BlockBody: Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + Debug
        + PartialEq
        + Eq
        + 'static;
    /// Full block type.
    type Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody>
        + Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + Debug
        + PartialEq
        + Eq
        + 'static;

    /// The transaction type which peers announce in `Transactions` messages. It is different from
    /// `PooledTransactions` to account for Ethereum case where EIP-4844 transactions are not being
    /// announced and can only be explicitly requested from peers.
    type BroadcastedTransaction: Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + Debug
        + PartialEq
        + Eq
        + 'static;
    /// The transaction type which peers return in `PooledTransactions` messages.
    type PooledTransaction: Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + Debug
        + PartialEq
        + Eq
        + 'static;
}

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
}
