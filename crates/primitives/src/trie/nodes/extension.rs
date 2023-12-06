use super::{super::Nibbles, rlp_node};
use alloy_rlp::{BufMut, Encodable};
use smallvec::SmallVec;

/// An intermediate node that exists solely to compress the trie's paths. It contains a path segment
/// (a shared prefix of keys) and a single child pointer. Essentially, an extension node can be
/// thought of as a shortcut within the trie to reduce its overall depth.
///
/// The purpose of an extension node is to optimize the trie structure by collapsing multiple nodes
/// with a single child into one node. This simplification reduces the space and computational
/// complexity when performing operations on the trie.
pub struct ExtensionNode<'a> {
    /// A common prefix for keys. See [`Nibbles::encode_path_leaf`] for more information.
    pub prefix: SmallVec<[u8; 36]>,
    /// A pointer to the child.
    pub node: &'a [u8],
}

impl<'a> ExtensionNode<'a> {
    /// Creates a new extension node with the given prefix and child.
    pub fn new(prefix: &Nibbles, node: &'a [u8]) -> Self {
        Self { prefix: prefix.encode_path_leaf(false), node }
    }

    /// RLP encodes the node and returns either RLP(Node) or RLP(keccak(RLP(node))).
    pub fn rlp(&self, buf: &mut Vec<u8>) -> Vec<u8> {
        self.encode(buf);
        rlp_node(buf)
    }
}

impl Encodable for ExtensionNode<'_> {
    fn encode(&self, out: &mut dyn BufMut) {
        let h = alloy_rlp::Header {
            list: true,
            payload_length: self.prefix.as_slice().length() + self.node.len(),
        };
        h.encode(out);
        // Slices have different RLP encoding from Vectors so we need to `as_slice()
        self.prefix.as_slice().encode(out);
        // The nodes are already RLP encoded
        out.put_slice(self.node);
    }
}

impl std::fmt::Debug for ExtensionNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionNode")
            .field("prefix", &crate::hex::encode(&self.prefix))
            .field("node", &crate::hex::encode(self.node))
            .finish()
    }
}
