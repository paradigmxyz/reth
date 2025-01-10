//! Abstraction over primitive types in network messages.

use alloy_consensus::{RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt};
use alloy_rlp::{Decodable, Encodable};
use core::fmt::Debug;
use reth_primitives::NodePrimitives;
use reth_primitives_traits::{Block, BlockBody, BlockHeader, SignedTransaction};

/// Abstraction over primitive types which might appear in network messages. See
/// [`crate::EthMessage`] for more context.
pub trait NetworkPrimitives:
    Send + Sync + Unpin + Clone + Debug + PartialEq + Eq + 'static
{
    /// The block header type.
    type BlockHeader: BlockHeader + 'static;

    /// The block body type.
    type BlockBody: BlockBody + 'static;

    /// Full block type.
    type Block: Block<Header = Self::BlockHeader, Body = Self::BlockBody>
        + Encodable
        + Decodable
        + 'static;

    /// The transaction type which peers announce in `Transactions` messages. It is different from
    /// `PooledTransactions` to account for Ethereum case where EIP-4844 transactions are not being
    /// announced and can only be explicitly requested from peers.
    type BroadcastedTransaction: SignedTransaction + 'static;

    /// The transaction type which peers return in `PooledTransactions` messages.
    type PooledTransaction: SignedTransaction + TryFrom<Self::BroadcastedTransaction> + 'static;

    /// The transaction type which peers return in `GetReceipts` messages.
    type Receipt: TxReceipt + RlpEncodableReceipt + RlpDecodableReceipt + Unpin + 'static;
}

/// This is a helper trait for use in bounds, where some of the [`NetworkPrimitives`] associated
/// types must be the same as the [`NodePrimitives`] associated types.
pub trait NetPrimitivesFor<N: NodePrimitives>:
    NetworkPrimitives<
    BlockHeader = N::BlockHeader,
    BlockBody = N::BlockBody,
    Block = N::Block,
    Receipt = N::Receipt,
>
{
}

impl<N, T> NetPrimitivesFor<N> for T
where
    N: NodePrimitives,
    T: NetworkPrimitives<
        BlockHeader = N::BlockHeader,
        BlockBody = N::BlockBody,
        Block = N::Block,
        Receipt = N::Receipt,
    >,
{
}

/// Network primitive types used by Ethereum networks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EthNetworkPrimitives;

impl NetworkPrimitives for EthNetworkPrimitives {
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = reth_primitives::BlockBody;
    type Block = reth_primitives::Block;
    type BroadcastedTransaction = reth_primitives::TransactionSigned;
    type PooledTransaction = reth_primitives::PooledTransaction;
    type Receipt = reth_primitives::Receipt;
}
