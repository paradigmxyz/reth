use crate::{Block, BlockHeader, SignedTransaction};
use alloy_rlp::{Decodable, Encodable};
use core::fmt;

/// Abstraction over primitive types which might appear in network messages. See `EthMessage` for
/// more context.
pub trait NetworkPrimitives:
    Send + Sync + Unpin + Clone + fmt::Debug + PartialEq + Eq + 'static
{
    /// The block header type.
    type BlockHeader: BlockHeader
        + Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + fmt::Debug
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
        + fmt::Debug
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
        + fmt::Debug
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
        + fmt::Debug
        + PartialEq
        + Eq
        + 'static;

    /// The transaction type which peers return in `PooledTransactions` messages.
    type PooledTransaction: SignedTransaction + TryFrom<Self::BroadcastedTransaction> + 'static;

    /// The transaction type which peers return in `GetReceipts` messages.
    type Receipt: Encodable
        + Decodable
        + Send
        + Sync
        + Unpin
        + Clone
        + fmt::Debug
        + PartialEq
        + Eq
        + 'static;
}
