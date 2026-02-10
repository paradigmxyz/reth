//! Implements the `GetBlockAccessLists` and `BlockAccessLists` message types.

use alloc::vec::Vec;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::B256;
use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs_derive::add_arbitrary_tests;

/// A request for block access lists from the given block hashes.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockAccessLists(
    /// The block hashes to request block access lists for.
    pub Vec<B256>,
);

/// Response for [`GetBlockAccessLists`] containing one BAL per requested block hash.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct BlockAccessLists(
    /// The requested block access lists. Unavailable entries are represented by empty BALs.
    pub Vec<BlockAccessList>,
);
