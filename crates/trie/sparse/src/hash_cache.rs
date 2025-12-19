use alloy_primitives::{keccak256, B256};
use reth_trie_common::RlpNode;

#[cfg(feature = "std")]
use fixed_cache::Cache;
#[cfg(feature = "std")]
use std::sync::OnceLock;

// ~1M entries targets ~128 MiB total usage (key vecs + hashes + bucket overhead) while providing
// good reuse for repeated trie node RLPs during sparse trie hashing.
const TRIE_NODE_HASH_CACHE_SIZE: usize = 1 << 20;

#[cfg(feature = "std")]
fn trie_node_hash_cache() -> &'static Cache<Vec<u8>, B256> {
    static CACHE: OnceLock<Cache<Vec<u8>, B256>> = OnceLock::new();
    CACHE.get_or_init(|| Cache::new(TRIE_NODE_HASH_CACHE_SIZE, Default::default()))
}

/// Hashes an RLP-encoded trie node with a fixed-size cache.
pub fn hash_trie_node_cached(rlp: &[u8]) -> B256 {
    #[cfg(feature = "std")]
    {
        let cache = trie_node_hash_cache();
        if let Some(hash) = cache.get(rlp) {
            return hash;
        }

        let hash = keccak256(rlp);
        cache.insert(rlp.to_vec(), hash);
        return hash;
    }

    #[cfg(not(feature = "std"))]
    {
        keccak256(rlp)
    }
}

/// Returns `rlp(node)` for short encodings or `rlp(keccak(rlp(node)))` using the cache.
pub fn rlp_node_from_rlp_cached(rlp: &[u8]) -> RlpNode {
    if rlp.len() < 32 {
        RlpNode::from_raw(rlp).expect("RLP node length already checked")
    } else {
        let hash = hash_trie_node_cached(rlp);
        RlpNode::word_rlp(&hash)
    }
}
