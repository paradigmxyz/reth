use core::{fmt, mem::MaybeUninit};

use alloy_primitives::hex;
use alloy_rlp::{length_of_length, BufMut, Encodable, Header};
use reth_trie_common::{ExtensionNode, LeafNode, RlpNode};
use smallvec::SmallVec;

use crate::PackedNibbles;

/// Reference to the leaf node. See [LeafNode] from more information.
pub struct LeafNodeRef<'a> {
    /// The key for this leaf node.
    pub key: &'a PackedNibbles,
    /// The node value.
    pub value: &'a [u8],
}

impl fmt::Debug for LeafNodeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeafNodeRef")
            .field("key", &self.key)
            .field("value", &hex::encode(self.value))
            .finish()
    }
}

/// Manual implementation of encoding for the leaf node of Merkle Patricia Trie.
impl Encodable for LeafNodeRef<'_> {
    #[inline]
    fn encode(&self, out: &mut dyn BufMut) {
        Header { list: true, payload_length: self.rlp_payload_length() }.encode(out);
        encode_path_leaf(self.key, true).as_slice().encode(out);
        self.value.encode(out);
    }

    #[inline]
    fn length(&self) -> usize {
        let payload_length = self.rlp_payload_length();
        payload_length + length_of_length(payload_length)
    }
}

impl<'a> LeafNodeRef<'a> {
    /// Creates a new leaf node with the given key and value.
    pub const fn new(key: &'a PackedNibbles, value: &'a [u8]) -> Self {
        Self { key, value }
    }

    /// RLP-encodes the node and returns either `rlp(node)` or `rlp(keccak(rlp(node)))`.
    #[inline]
    pub fn rlp(&self, rlp: &mut Vec<u8>) -> RlpNode {
        self.encode(rlp);
        RlpNode::from_rlp(rlp)
    }

    /// Returns the length of RLP encoded fields of leaf node.
    #[inline]
    fn rlp_payload_length(&self) -> usize {
        let mut encoded_key_len = self.key.len() / 2 + 1;
        // For leaf nodes the first byte cannot be greater than 0x80.
        if encoded_key_len != 1 {
            encoded_key_len += length_of_length(encoded_key_len);
        }
        encoded_key_len + Encodable::length(&self.value)
    }
}

/// Reference to the extension node. See [ExtensionNode] from more information.
pub struct ExtensionNodeRef<'a> {
    /// The key for this extension node.
    pub key: &'a PackedNibbles,
    /// A pointer to the child node.
    pub child: &'a [u8],
}

impl fmt::Debug for ExtensionNodeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionNodeRef")
            .field("key", &self.key)
            .field("node", &hex::encode(self.child))
            .finish()
    }
}

impl Encodable for ExtensionNodeRef<'_> {
    #[inline]
    fn encode(&self, out: &mut dyn BufMut) {
        Header { list: true, payload_length: self.rlp_payload_length() }.encode(out);
        encode_path_leaf(self.key, false).as_slice().encode(out);
        // Pointer to the child is already RLP encoded.
        out.put_slice(self.child);
    }

    #[inline]
    fn length(&self) -> usize {
        let payload_length = self.rlp_payload_length();
        payload_length + length_of_length(payload_length)
    }
}

impl<'a> ExtensionNodeRef<'a> {
    /// Creates a new extension node with the given key and a pointer to the child.
    #[inline]
    pub const fn new(key: &'a PackedNibbles, child: &'a [u8]) -> Self {
        Self { key, child }
    }

    /// RLP-encodes the node and returns either `rlp(node)` or `rlp(keccak(rlp(node)))`.
    #[inline]
    pub fn rlp(&self, rlp: &mut Vec<u8>) -> RlpNode {
        self.encode(rlp);
        RlpNode::from_rlp(rlp)
    }

    /// Returns the length of RLP encoded fields of extension node.
    #[inline]
    fn rlp_payload_length(&self) -> usize {
        let mut encoded_key_len = self.key.len() / 2 + 1;
        // For extension nodes the first byte cannot be greater than 0x80.
        if encoded_key_len != 1 {
            encoded_key_len += length_of_length(encoded_key_len);
        }
        encoded_key_len + self.child.len()
    }
}

pub fn encode_path_leaf(nibbles: &PackedNibbles, is_leaf: bool) -> SmallVec<[u8; 36]> {
    let odd_nibbles = nibbles.len() % 2 != 0;
    let encoded_len = nibbles.len() / 2 + 1;
    // SAFETY: `len` is calculated correctly.
    unsafe {
        nybbles::smallvec_with(encoded_len, |buf| {
            let (first, rest) = buf.split_first_mut().unwrap_unchecked();
            first.write(match (is_leaf, odd_nibbles) {
                (true, true) => LeafNode::ODD_FLAG | nibbles.get_nibble(0),
                (true, false) => LeafNode::EVEN_FLAG,
                (false, true) => ExtensionNode::ODD_FLAG | nibbles.get_nibble(0),
                (false, false) => ExtensionNode::EVEN_FLAG,
            });
            if odd_nibbles {
                pack_to_unchecked(&nibbles.slice(1..), rest);
            } else {
                pack_to_unchecked(nibbles, rest);
            }
        })
    }
}

/// Packs the nibbles into the given slice without checking its length.
///
/// # Safety
///
/// `out` must be valid for at least `(self.len() + 1) / 2` bytes.
#[inline]
pub unsafe fn pack_to_unchecked(nibbles: &PackedNibbles, out: &mut [MaybeUninit<u8>]) {
    let len = nibbles.len();
    debug_assert!(out.len() >= len.div_ceil(2));
    let ptr = out.as_mut_ptr().cast::<u8>();
    // TODO: just take as_le_slice of U256
    for i in 0..len / 2 {
        let high_nibble = nibbles.get_nibble(i * 2);
        let low_nibble = nibbles.get_nibble(i * 2 + 1);
        ptr.add(i).write((high_nibble << 4) | low_nibble);
    }
    if len % 2 != 0 {
        let i = len / 2;
        ptr.add(i).write(nibbles.last().unwrap_unchecked() << 4);
    }
}
