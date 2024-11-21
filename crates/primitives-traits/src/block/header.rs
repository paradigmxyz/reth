//! Block header data primitive.

use core::fmt;

use alloy_primitives::Sealable;

use crate::{InMemorySize, MaybeArbitrary, MaybeCompact, MaybeSerde};

/// Helper trait that unifies all behaviour required by block header to support full node
/// operations.
pub trait FullBlockHeader: BlockHeader + MaybeCompact {}

impl<T> FullBlockHeader for T where T: BlockHeader + MaybeCompact {}

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
    + MaybeSerde
    + MaybeArbitrary
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
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
        + alloy_consensus::BlockHeader
        + Sealable
        + InMemorySize
        + MaybeSerde
        + MaybeArbitrary
{
}
