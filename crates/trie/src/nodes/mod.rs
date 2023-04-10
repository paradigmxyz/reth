use reth_primitives::{keccak256, H256};
use reth_rlp::EMPTY_STRING_CODE;

mod branch;
pub use branch::BranchNode;

mod extension;
pub use extension::ExtensionNode;

mod leaf;
pub use leaf::LeafNode;

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

fn matches_mask(mask: u16, idx: i32) -> bool {
    mask & (1u16 << idx) != 0
}
