//! Block header data primitive.

use core::fmt;

use alloy_primitives::Sealable;
use reth_codecs::Compact;

use crate::InMemorySize;

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
    + InMemorySize
{
}

impl<T> BlockHeader for T where
    T: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + serde::Serialize
        + for<'de> serde::Deserialize<'de>
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
        + alloy_consensus::BlockHeader
        + Sealable
        + InMemorySize
{
}
