//! Block abstraction.

pub mod body;
pub mod header;

use alloc::fmt;

use crate::{
    BlockBody, BlockHeader, FullBlockBody, FullBlockHeader, InMemorySize, MaybeArbitrary,
    MaybeSerde,
};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock:
    Block<Header: FullBlockHeader, Body: FullBlockBody> + alloy_rlp::Encodable + alloy_rlp::Decodable
{
}

impl<T> FullBlock for T where
    T: Block<Header: FullBlockHeader, Body: FullBlockBody>
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
{
}

/// Abstraction of block data type.
// todo: make sealable super-trait, depends on <https://github.com/paradigmxyz/reth/issues/11449>
// todo: make with senders extension trait, so block can be impl by block type already containing
// senders
pub trait Block:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + InMemorySize
    + MaybeSerde
    + MaybeArbitrary
{
    /// Header part of the block.
    type Header: BlockHeader + 'static;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody<OmmerHeader = Self::Header> + Send + Sync + Unpin + 'static;

    /// Create new block instance.
    fn new(header: Self::Header, body: Self::Body) -> Self;

    /// Returns reference to block header.
    fn header(&self) -> &Self::Header;

    /// Returns reference to block body.
    fn body(&self) -> &Self::Body;

    /// Splits the block into its header and body.
    fn split(self) -> (Self::Header, Self::Body);
}
