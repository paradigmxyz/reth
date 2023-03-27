use super::nibbles::Nibbles;
use reth_primitives::{bytes::BytesMut, keccak256, H256};
use reth_rlp::{BufMut, Encodable, EMPTY_STRING_CODE};

pub const KECCAK_LENGTH: usize = H256::len_bytes();

fn assert_subset(sub: u16, sup: u16) {
    assert_eq!(sub & sup, sub);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntermediateNode {
    pub state_mask: u16,
    pub tree_mask: u16,
    pub hash_mask: u16,
    pub hashes: Vec<H256>,
    pub root_hash: Option<H256>,
}

impl IntermediateNode {
    pub fn new(
        state_mask: u16,
        tree_mask: u16,
        hash_mask: u16,
        hashes: Vec<H256>,
        root_hash: Option<H256>,
    ) -> Self {
        assert_subset(tree_mask, state_mask);
        assert_subset(hash_mask, state_mask);
        assert_eq!(hash_mask.count_ones() as usize, hashes.len());
        Self { state_mask, tree_mask, hash_mask, hashes, root_hash }
    }

    pub fn hash_for_nibble(&self, nibble: i8) -> H256 {
        let mask = (1u16 << nibble) - 1;
        let index = (self.hash_mask & mask).count_ones();
        self.hashes[index as usize]
    }

    pub fn marshal(&self) -> Vec<u8> {
        let n = self;
        let number_of_hashes = n.hashes.len() + usize::from(n.root_hash.is_some());
        let buf_size = number_of_hashes * KECCAK_LENGTH + 6;
        let mut buf = Vec::<u8>::with_capacity(buf_size);

        buf.extend_from_slice(n.state_mask.to_be_bytes().as_slice());
        buf.extend_from_slice(n.tree_mask.to_be_bytes().as_slice());
        buf.extend_from_slice(n.hash_mask.to_be_bytes().as_slice());

        if let Some(root_hash) = n.root_hash {
            buf.extend_from_slice(root_hash.as_bytes());
        }

        for hash in &n.hashes {
            buf.extend_from_slice(hash.as_bytes());
        }

        buf
    }

    pub fn unmarshal<T: AsRef<[u8]>>(v: T) -> Option<Self> {
        let v = v.as_ref();
        if v.len() % KECCAK_LENGTH != 6 {
            return None
        }

        let state_mask = u16::from_be_bytes(v[0..2].try_into().unwrap());
        let tree_mask = u16::from_be_bytes(v[2..4].try_into().unwrap());
        let hash_mask = u16::from_be_bytes(v[4..6].try_into().unwrap());
        let mut i = 6;

        let mut root_hash = None;
        if hash_mask.count_ones() as usize + 1 == v[6..].len() / KECCAK_LENGTH {
            root_hash = Some(H256::from_slice(&v[i..i + KECCAK_LENGTH]));
            i += KECCAK_LENGTH;
        }

        let num_hashes = v[i..].len() / KECCAK_LENGTH;
        let mut hashes = Vec::<H256>::with_capacity(num_hashes);
        for _ in 0..num_hashes {
            hashes.push(H256::from_slice(&v[i..i + KECCAK_LENGTH]));
            i += KECCAK_LENGTH;
        }

        Some(Self::new(state_mask, tree_mask, hash_mask, hashes, root_hash))
    }
}

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
        #[derive(reth_rlp::RlpEncodable)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    // From manual regression test
    #[test]
    fn encode_leaf_node_nibble() {
        let nibble = Nibbles { hex_data: hex!("0604060f").to_vec() };
        let encoded = nibble.encode_path_leaf(true);
        let expected = hex!("20646f").to_vec();
        assert_eq!(encoded, expected);
    }

    #[test]
    fn rlp_leaf_node_roundtrip() {
        let nibble = Nibbles { hex_data: hex!("0604060f").to_vec() };
        let val = hex!("76657262").to_vec();
        let leaf = LeafNode::new(&nibble, &val);
        let rlp = leaf.rlp();

        let expected = hex!("c98320646f8476657262").to_vec();
        assert_eq!(rlp, expected);
    }

    #[test]
    fn node_marshalling() {
        let n = IntermediateNode::new(
            0xf607,
            0x0005,
            0x4004,
            vec![
                hex!("90d53cd810cc5d4243766cd4451e7b9d14b736a1148b26b3baac7617f617d321").into(),
                hex!("cc35c964dda53ba6c0b87798073a9628dbc9cd26b5cce88eb69655a9c609caf1").into(),
            ],
            Some(hex!("aaaabbbb0006767767776fffffeee44444000005567645600000000eeddddddd").into()),
        );

        assert_eq!(IntermediateNode::unmarshal(&n.marshal()).unwrap(), n);
    }
}
