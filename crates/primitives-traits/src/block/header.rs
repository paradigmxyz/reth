//! Block header data primitive.

use core::{fmt, hash::Hash};

use alloy_primitives::Sealable;

use crate::{InMemorySize, MaybeCompact, MaybeSerde, MaybeSerdeBincodeCompat};

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
    + Hash
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
    + MaybeSerdeBincodeCompat
    + AsRef<Self>
    + 'static
{
}

impl BlockHeader for alloy_consensus::Header {}
