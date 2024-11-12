//! Block header data primitive.

use core::fmt;

use alloy_primitives::Sealable;
use reth_codecs::Compact;

/// Helper trait that unifies all behaviour required by block header to support full node
/// operations.
pub trait FullBlockHeader: BlockHeader + Compact {}

impl<T> FullBlockHeader for T where T: BlockHeader + Compact {}

/// Abstraction of a block header.
pub trait BlockHeader:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + alloy_consensus::BlockHeader
    + Sealable
{
    /// Calculates a heuristic for the in-memory size of the [`BlockHeader`].
    fn size(&self) -> usize;
}

impl BlockHeader for alloy_consensus::Header {
    fn size(&self) -> usize {
        self.size()
    }
}
