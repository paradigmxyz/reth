use alloy_primitives::keccak256_uncached;
use alloy_trie::nodes::RlpNode;

/// Returns an RLP-encoded trie node as an [`RlpNode`]: the encoding itself if it is shorter than
/// 32 bytes, otherwise the RLP of its hash.
///
/// Unlike [`RlpNode::from_rlp`], this never goes through alloy's global keccak cache. A trie node
/// is hashed once per change and rarely again, so caching it only evicts hashes that the EVM
/// computes again and again, such as the storage slots of mappings.
pub fn rlp_node_uncached(rlp: &[u8]) -> RlpNode {
    if rlp.len() < 32 {
        RlpNode::from_raw(rlp).expect("an encoding shorter than 32 bytes fits an RlpNode")
    } else {
        RlpNode::word_rlp(&keccak256_uncached(rlp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlp_node_uncached_matches_rlp_node_from_rlp() {
        for len in [0, 1, 31, 32, 33, 87, 88, 532] {
            let rlp = (0..len).map(|i| i as u8).collect::<Vec<_>>();
            assert_eq!(rlp_node_uncached(&rlp), RlpNode::from_rlp(&rlp), "length {len}");
        }
    }
}
