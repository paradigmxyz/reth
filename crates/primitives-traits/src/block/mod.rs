//! Block abstraction.

pub mod body;
pub mod header;

use alloc::fmt;

use alloy_primitives::B256;
use reth_codecs::Compact;

use crate::{BlockBody, BlockHeader, FullBlockHeader};

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
    + From<(Self::Header, Self::Body)>
    + Into<(Self::Header, Self::Body)>
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody;

    /// A block and block hash.
    type SealedBlock<H, B>;

    /// A block and addresses of senders of transactions in it.
    type BlockWithSenders<T>;

    /// Returns reference to [`BlockHeader`] type.
    fn header(&self) -> &Self::Header;

    /// Returns reference to [`BlockBody`] type.
    fn body(&self) -> &Self::Body;

    /// Calculate the header hash and seal the block so that it can't be changed.
    // todo: can be default impl if sealed block type is made generic over header and body and
    // migrated to alloy
    fn seal_slow(self) -> Self::SealedBlock<Self::Header, Self::Body>;

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    // todo: can be default impl if sealed block type is made generic over header and body and
    // migrated to alloy
    fn seal(self, hash: B256) -> Self::SealedBlock<Self::Header, Self::Body>;

    /// Calculates a heuristic for the in-memory size of the [`Block`].
    fn size(&self) -> usize;
}
