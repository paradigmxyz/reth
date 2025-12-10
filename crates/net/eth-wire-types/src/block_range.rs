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
