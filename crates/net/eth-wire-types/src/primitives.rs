//! Abstraction over primitive types in network messages.

use crate::NewBlockPayload;
use alloy_consensus::{RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt};
use alloy_rlp::{Decodable, Encodable};
use core::fmt::Debug;
use reth_ethereum_primitives::{EthPrimitives, PooledTransactionVariant};
use reth_primitives_traits::{
    Block, BlockBody, BlockHeader, BlockTy, NodePrimitives, SignedTransaction,
};

/// Abstraction over primitive types which might appear in network messages. See
/// [`crate::EthMessage`] for more context.
pub trait NetworkPrimitives: Send + Sync + Unpin + Clone + Debug + 'static {
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
    type Receipt: TxReceipt
        + RlpEncodableReceipt
        + RlpDecodableReceipt
        + Encodable
        + Decodable
        + Unpin
        + 'static;

    /// The payload type for the `NewBlock` message.
    type NewBlockPayload: NewBlockPayload<Block = Self::Block>;
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

/// Basic implementation of [`NetworkPrimitives`] combining [`NodePrimitives`] and a pooled
/// transaction.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicNetworkPrimitives<N: NodePrimitives, Pooled, NewBlock = crate::NewBlock<BlockTy<N>>>(
    core::marker::PhantomData<(N, Pooled, NewBlock)>,
);

impl<N, Pooled, NewBlock> NetworkPrimitives for BasicNetworkPrimitives<N, Pooled, NewBlock>
where
    N: NodePrimitives,
    Pooled: SignedTransaction + TryFrom<N::SignedTx> + 'static,
    NewBlock: NewBlockPayload<Block = N::Block>,
{
    type BlockHeader = N::BlockHeader;
    type BlockBody = N::BlockBody;
    type Block = N::Block;
    type BroadcastedTransaction = N::SignedTx;
    type PooledTransaction = Pooled;
    type Receipt = N::Receipt;
    type NewBlockPayload = NewBlock;
}

/// Network primitive types used by Ethereum networks.
pub type EthNetworkPrimitives = BasicNetworkPrimitives<EthPrimitives, PooledTransactionVariant>;
