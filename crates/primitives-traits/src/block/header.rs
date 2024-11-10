use core::fmt;

use alloy_primitives::Sealable;
use reth_codecs::Compact;

/// Helper trait that unifies all behaviour required by header to support full node operations.
pub trait FullBlockHeader: BlockHeader + Compact {}

impl<T> FullBlockHeader for T where T: BlockHeader + Compact {}

pub trait BlockHeader:
    Send
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
{
}
