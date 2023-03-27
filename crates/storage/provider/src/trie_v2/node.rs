use super::nibbles::Nibbles;
use reth_primitives::{bytes::BytesMut, keccak256, H256};
use reth_rlp::{BufMut, Encodable, EMPTY_STRING_CODE};

pub const KECCAK_LENGTH: usize = H256::len_bytes();

#[derive(Default)]
pub struct LeafNode<'a> {
    /// Compact RLP encoded Nibbles
    pub key: Vec<u8>,
    /// value: SmallVec<[u8; 36]>
    pub value: &'a [u8],
}

impl<'a> LeafNode<'a> {
    pub fn new(key: &Nibbles, value: &'a [u8]) -> Self {
        Self { key: key.encode_path_leaf(true), value }
    }

    /// RLP encodes the node and returns either RLP(Node) or RLP(keccak(RLP(node)))
    /// depending on if the serialized node was longer than a keccak).
    pub fn rlp(&self) -> Vec<u8> {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        rlp_node(&out)
    }
}

// Handroll because `key` must be encoded as a slice
impl Encodable for LeafNode<'_> {
    fn encode(&self, out: &mut dyn BufMut) {
        let h = reth_rlp::Header {
            list: true,
            payload_length: self.key.as_slice().length() + self.value.len(),
        };
        h.encode(out);
        self.key.as_slice().encode(out);
        self.value.encode(out);
    }
}

impl std::fmt::Debug for LeafNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafNode")
            .field("key", &hex::encode(&&self.key))
            .field("value", &hex::encode(&self.value))
            .finish()
    }
}

pub struct ExtensionNode<'a> {
    pub prefix: Vec<u8>,
    pub node: &'a [u8],
}

impl<'a> ExtensionNode<'a> {
    pub fn new(prefix: &Nibbles, node: &'a [u8]) -> Self {
        Self { prefix: prefix.encode_path_leaf(false), node }
    }

    pub fn rlp(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        self.encode(&mut buf);
        rlp_node(&buf)
    }
}

impl Encodable for ExtensionNode<'_> {
    fn encode(&self, out: &mut dyn BufMut) {
        let h = reth_rlp::Header {
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
            .field("prefix", &hex::encode(&&self.prefix))
            .field("node", &hex::encode(&self.node))
            .finish()
    }
}

#[derive(Clone, Debug)]
/// A Branch node is only a pointer to the stack of nodes and is used to
/// create the RLP encoding of the node using masks which filter from
/// the stack of nodes.
pub struct BranchNode<'a> {
    /// Rlp encoded children
    pub stack: &'a [Vec<u8>],
}

impl<'a> BranchNode<'a> {
    pub fn new(stack: &'a [Vec<u8>]) -> Self {
        Self { stack }
    }

    /// Given the hash and state mask of children present, return an iterator over the stack items
    /// that match the mask.
    // TODO: Test this!
    pub fn children(
        &self,
        state_mask: u16,
        hash_mask: u16,
    ) -> impl Iterator<Item = &'a Vec<u8>> + '_ {
        let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;
        self.stack.iter().skip(first_child_idx).enumerate().filter_map(move |(idx, item)| {
            let idx = first_child_idx + idx;
            if matches_mask(state_mask, idx) && matches_mask(hash_mask, idx) {
                Some(item)
            } else {
                None
            }
        })
    }

    /// Returns the RLP encoding of the branch node given the state mask of children present.
    pub fn rlp(&self, state_mask: u16) -> Vec<u8> {
        let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;
        let mut buf = BytesMut::new();

        // Create the RLP header from the mask elements present.
        let mut i = first_child_idx;
        let header = (0..16).fold(
            reth_rlp::Header { list: true, payload_length: 1 },
            |mut header, digit| {
                if matches_mask(state_mask, digit) {
                    header.payload_length += self.stack[i].len();
                    i += 1;
                } else {
                    header.payload_length += 1;
                }
                header
            },
        );
        header.encode(&mut buf);

        // Extend the RLP buffer with the present children
        let mut i = first_child_idx;
        (0..16).for_each(|idx| {
            if matches_mask(state_mask, idx) {
                buf.extend_from_slice(&self.stack[i]);
                i += 1;
            } else {
                buf.put_u8(EMPTY_STRING_CODE)
            }
        });

        // Is this needed?
        buf.put_u8(EMPTY_STRING_CODE);

        rlp_node(&buf)
    }
}

/// Given an RLP encoded node, returns either RLP(Node) or RLP(keccak(RLP(node)))
fn rlp_node(rlp: &[u8]) -> Vec<u8> {
    if rlp.len() < KECCAK_LENGTH {
        rlp.to_vec()
    } else {
        rlp_hash(keccak256(rlp))
    }
}

// Optimization for quick encoding of a hash as RLP
pub fn rlp_hash(hash: H256) -> Vec<u8> {
    [[EMPTY_STRING_CODE + KECCAK_LENGTH as u8].as_slice(), hash.0.as_slice()].concat()
}

fn matches_mask(mask: u16, idx: usize) -> bool {
    mask & (1 << idx) != 0
}
