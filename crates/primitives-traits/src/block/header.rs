//! Block header data primitive.

use crate::{InMemorySize, MaybeCompact, MaybeSerde, MaybeSerdeBincodeCompat};
use alloy_primitives::{Bytes, Sealable};
use core::{fmt, hash::Hash};

/// Re-exported alias
pub use alloy_consensus::BlockHeader as AlloyBlockHeader;

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
    + BlockHeaderMut
{
}

impl BlockHeader for alloy_consensus::Header {}

/// Returns a mutable reference to fields of the header.
pub trait BlockHeaderMut {
    /// Mutable reference to the extra data.
    fn extra_data_mut(&mut self) -> &mut Bytes;
}

impl BlockHeaderMut for alloy_consensus::Header {
    fn extra_data_mut(&mut self) -> &mut Bytes {
        &mut self.extra_data
    }
}
