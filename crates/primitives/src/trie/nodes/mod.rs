use crate::{keccak256, B256};
use alloy_rlp::EMPTY_STRING_CODE;
use std::ops::Range;

mod branch;
pub use branch::{BranchNode, BranchNodeCompact};

mod extension;
pub use extension::ExtensionNode;

mod leaf;
pub use leaf::LeafNode;

/// The range of valid child indexes.
pub const CHILD_INDEX_RANGE: Range<u8> = 0..16;

/// Given an RLP encoded node, returns either RLP(node) or RLP(keccak(RLP(node)))
#[inline]
fn rlp_node(rlp: &[u8]) -> Vec<u8> {
    if rlp.len() < B256::len_bytes() {
        rlp.to_vec()
    } else {
        word_rlp(&keccak256(rlp))
    }
}

/// Optimization for quick encoding of a 32-byte word as RLP.
// TODO: this could return [u8; 33] but Vec is needed everywhere this function is used
#[inline]
pub fn word_rlp(word: &B256) -> Vec<u8> {
    // Gets optimized to alloc + write directly into it: https://godbolt.org/z/rfWGG6ebq
    let mut arr = [0; 33];
    arr[0] = EMPTY_STRING_CODE + 32;
    arr[1..].copy_from_slice(word.as_slice());
    arr.to_vec()
}
