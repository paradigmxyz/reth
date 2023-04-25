use super::{rlp_node, CHILD_INDEX_RANGE};
use reth_primitives::{trie::TrieMask, H256};
use reth_rlp::{BufMut, EMPTY_STRING_CODE};

/// A Branch node is only a pointer to the stack of nodes and is used to
/// create the RLP encoding of the node using masks which filter from
/// the stack of nodes.
#[derive(Clone, Debug)]
pub struct BranchNode<'a> {
    /// Rlp encoded children
    pub stack: &'a [Vec<u8>],
}

impl<'a> BranchNode<'a> {
    /// Create a new branch node from the stack of nodes.
    pub fn new(stack: &'a [Vec<u8>]) -> Self {
        Self { stack }
    }

    /// Given the hash and state mask of children present, return an iterator over the stack items
    /// that match the mask.
    pub fn children(
        &self,
        state_mask: TrieMask,
        hash_mask: TrieMask,
    ) -> impl Iterator<Item = H256> + '_ {
        let mut index = self.stack.len() - state_mask.count_ones() as usize;
        CHILD_INDEX_RANGE.filter_map(move |digit| {
            let mut child = None;
            if state_mask.is_bit_set(digit) {
                if hash_mask.is_bit_set(digit) {
                    child = Some(&self.stack[index]);
                }
                index += 1;
            }
            child.map(|child| H256::from_slice(&child[1..]))
        })
    }

    /// Returns the RLP encoding of the branch node given the state mask of children present.
    pub fn rlp(&self, state_mask: TrieMask, buf: &mut Vec<u8>) -> Vec<u8> {
        let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;

        // Create the RLP header from the mask elements present.
        let mut i = first_child_idx;
        let header = CHILD_INDEX_RANGE.fold(
            reth_rlp::Header { list: true, payload_length: 1 },
            |mut header, digit| {
                if state_mask.is_bit_set(digit) {
                    header.payload_length += self.stack[i].len();
                    i += 1;
                } else {
                    header.payload_length += 1;
                }
                header
            },
        );
        header.encode(buf);

        // Extend the RLP buffer with the present children
        let mut i = first_child_idx;
        CHILD_INDEX_RANGE.for_each(|idx| {
            if state_mask.is_bit_set(idx) {
                buf.extend_from_slice(&self.stack[i]);
                i += 1;
            } else {
                buf.put_u8(EMPTY_STRING_CODE)
            }
        });

        // Is this needed?
        buf.put_u8(EMPTY_STRING_CODE);

        rlp_node(buf)
    }
}
