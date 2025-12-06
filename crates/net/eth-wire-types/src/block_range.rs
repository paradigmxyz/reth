//! Block range related types introduced in eth/70 (EIP-7975).

use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs_derive::add_arbitrary_tests;

/// The block range a peer can currently serve (inclusive bounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct BlockRange {
    /// Earliest block number the peer can serve.
    pub start_block: u64,
    /// Latest block number the peer can serve.
    pub end_block: u64,
}

impl BlockRange {
    /// Returns true if the start/end pair forms a valid range.
    pub const fn is_valid(&self) -> bool {
        self.start_block <= self.end_block
    }
}

/// eth/70 request for the peer's current block range.
///
/// The payload is empty; the request id is carried by the [`crate::message::RequestPair`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct RequestBlockRange;

/// eth/70 response to [`RequestBlockRange`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct SendBlockRange {
    /// The earliest block which is available.
    pub start_block: u64,
    /// The latest block which is available.
    pub end_block: u64,
}

impl SendBlockRange {
    /// Returns the contained range.
    pub const fn as_range(&self) -> BlockRange {
        BlockRange { start_block: self.start_block, end_block: self.end_block }
    }

    /// Constructs from a [`BlockRange`].
    pub const fn from_range(range: BlockRange) -> Self {
        Self { start_block: range.start_block, end_block: range.end_block }
    }
}
