use reth_primitives::{keccak256, H256};
use reth_rlp::EMPTY_STRING_CODE;
use std::ops::Range;

mod branch;
mod extension;
mod leaf;

pub use self::{branch::BranchNode, extension::ExtensionNode, leaf::LeafNode};

/// The range of valid child indexes.
pub const CHILD_INDEX_RANGE: Range<u8> = 0..16;

/// Given an RLP encoded node, returns either RLP(Node) or RLP(keccak(RLP(node)))
fn rlp_node(rlp: &[u8]) -> Vec<u8> {
    if rlp.len() < H256::len_bytes() {
        rlp.to_vec()
    } else {
        rlp_hash(keccak256(rlp))
    }
}

/// Optimization for quick encoding of a hash as RLP
pub fn rlp_hash(hash: H256) -> Vec<u8> {
    [[EMPTY_STRING_CODE + H256::len_bytes() as u8].as_slice(), hash.0.as_slice()].concat()
}
