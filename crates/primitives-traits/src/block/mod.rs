//! Block abstraction.

pub mod body;
pub mod header;

use alloc::fmt;

use reth_codecs::Compact;

use crate::{BlockHeader, FullBlockHeader, InMemorySize};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock: Block<Header: Compact> + Compact {}

impl<T> FullBlock for T where T: Block<Header: FullBlockHeader> + Compact {}

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
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + InMemorySize
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block.
    type Body: Send + Sync + Unpin + 'static;

    /// Returns reference to block header.
    fn header(&self) -> &Self::Header;

    /// Returns reference to block body.
    fn body(&self) -> &Self::Body;
}
