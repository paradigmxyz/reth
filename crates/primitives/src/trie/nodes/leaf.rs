use super::{super::Nibbles, rlp_node};
use alloy_rlp::{BufMut, Encodable};
use smallvec::SmallVec;

/// A leaf node represents the endpoint or terminal node in the trie. In other words, a leaf node is
/// where actual values are stored.
///
/// A leaf node consists of two parts: the key (or path) and the value. The key is typically the
/// remaining portion of the key after following the path through the trie, and the value is the
/// data associated with the full key. When searching the trie for a specific key, reaching a leaf
/// node means that the search has successfully found the value associated with that key.
#[derive(Default)]
pub struct LeafNode<'a> {
    /// The key path. See [`Nibbles::encode_path_leaf`] for more information.
    pub key: SmallVec<[u8; 36]>,
    /// The node value.
    pub value: &'a [u8],
}

impl<'a> LeafNode<'a> {
    /// Creates a new leaf node with the given key and value.
    pub fn new(key: &Nibbles, value: &'a [u8]) -> Self {
        Self { key: key.encode_path_leaf(true), value }
    }

    /// RLP encodes the node and returns either RLP(Node) or RLP(keccak(RLP(node)))
    /// depending on if the serialized node was longer than a keccak).
    pub fn rlp(&self, out: &mut Vec<u8>) -> Vec<u8> {
        self.encode(out);
        rlp_node(out)
    }
}

// Handroll because `key` must be encoded as a slice
impl Encodable for LeafNode<'_> {
    fn encode(&self, out: &mut dyn BufMut) {
        #[derive(alloy_rlp::RlpEncodable)]
        struct S<'a> {
            encoded_path: &'a [u8],
            value: &'a [u8],
        }
        S { encoded_path: &self.key, value: self.value }.encode(out);
    }
}

impl std::fmt::Debug for LeafNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafNode")
            .field("key", &crate::hex::encode(&self.key))
            .field("value", &crate::hex::encode(self.value))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    // From manual regression test
    #[test]
    fn encode_leaf_node_nibble() {
        let nibble = Nibbles::from_nibbles_unchecked(hex!("0604060f"));
        let encoded = nibble.encode_path_leaf(true);
        assert_eq!(encoded[..], hex!("20646f"));
    }

    #[test]
    fn rlp_leaf_node_roundtrip() {
        let nibble = Nibbles::from_nibbles_unchecked(hex!("0604060f"));
        let val = hex!("76657262");
        let leaf = LeafNode::new(&nibble, &val);
        let rlp = leaf.rlp(&mut vec![]);
        assert_eq!(rlp, hex!("c98320646f8476657262"));
    }
}
