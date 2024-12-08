use super::{
    BRANCH_NODE_LBRB_DOMAIN, BRANCH_NODE_LBRT_DOMAIN, BRANCH_NODE_LTRB_DOMAIN,
    BRANCH_NODE_LTRT_DOMAIN,
};
use alloy_primitives::{hex, B256};
use alloy_trie::TrieMask;
use core::{fmt, ops::Range, slice::Iter};
use reth_scroll_primitives::poseidon::{hash_with_domain, Fr, PrimeField};

#[allow(unused_imports)]
use alloc::vec::Vec;

/// The range of valid child indexes.
pub(crate) const CHILD_INDEX_RANGE: Range<u8> = 0..2;

/// A trie mask to extract the two child indexes from a branch node.
pub(crate) const CHILD_INDEX_MASK: TrieMask = TrieMask::new(0b11);

/// A reference to branch node and its state mask.
/// NOTE: The stack may contain more items that specified in the state mask.
#[derive(Clone)]
pub(crate) struct BranchNodeRef<'a> {
    /// Reference to the collection of hash nodes.
    /// NOTE: The referenced stack might have more items than the number of children
    /// for this node. We should only ever access items starting from
    /// [`BranchNodeRef::first_child_index`].
    pub stack: &'a [B256],
    /// Reference to bitmask indicating the presence of children at
    /// the respective nibble positions.
    pub state_mask: TrieMask,
}

impl fmt::Debug for BranchNodeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BranchNodeRef")
            .field("stack", &self.stack.iter().map(hex::encode).collect::<Vec<_>>())
            .field("state_mask", &self.state_mask)
            .field("first_child_index", &self.first_child_index())
            .finish()
    }
}

impl<'a> BranchNodeRef<'a> {
    /// Create a new branch node from the stack of nodes.
    #[inline]
    pub(crate) const fn new(stack: &'a [B256], state_mask: TrieMask) -> Self {
        Self { stack, state_mask }
    }

    /// Returns the stack index of the first child for this node.
    ///
    /// # Panics
    ///
    /// If the stack length is less than number of children specified in state mask.
    /// Means that the node is in inconsistent state.
    #[inline]
    pub(crate) fn first_child_index(&self) -> usize {
        self.stack
            .len()
            .checked_sub((self.state_mask & CHILD_INDEX_MASK).count_ones() as usize)
            .expect("branch node stack is in inconsistent state")
    }

    #[inline]
    fn children(&self) -> impl Iterator<Item = (u8, Option<&B256>)> + '_ {
        BranchChildrenIter::new(self)
    }

    /// Given the hash mask of children, return an iterator over stack items
    /// that match the mask.
    #[inline]
    pub(crate) fn child_hashes(&self, hash_mask: TrieMask) -> impl Iterator<Item = B256> + '_ {
        self.children()
            .filter_map(|(i, c)| c.map(|c| (i, c)))
            .filter(move |(index, _)| hash_mask.is_bit_set(*index))
            .map(|(_, child)| B256::from_slice(&child[..]))
    }

    pub(crate) fn hash(&self) -> B256 {
        let mut children_iter = self.children();

        let left_child = children_iter
            .next()
            .map(|(_, c)| *c.unwrap_or_default())
            .expect("branch node has two children");
        let left_child =
            Fr::from_repr_vartime(left_child.0).expect("left child is a valid field element");
        let right_child = children_iter
            .next()
            .map(|(_, c)| *c.unwrap_or_default())
            .expect("branch node has two children");
        let right_child =
            Fr::from_repr_vartime(right_child.0).expect("right child is a valid field element");

        hash_with_domain(&[left_child, right_child], self.hashing_domain()).to_repr().into()
    }

    fn hashing_domain(&self) -> Fr {
        match *self.state_mask {
            0b1011 => BRANCH_NODE_LBRT_DOMAIN,
            0b1111 => BRANCH_NODE_LTRT_DOMAIN,
            0b0111 => BRANCH_NODE_LTRB_DOMAIN,
            0b0011 => BRANCH_NODE_LBRB_DOMAIN,
            _ => unreachable!("invalid branch node state mask"),
        }
    }
}

/// Iterator over branch node children.
#[derive(Debug)]
struct BranchChildrenIter<'a> {
    range: Range<u8>,
    state_mask: TrieMask,
    stack_iter: Iter<'a, B256>,
}

impl<'a> BranchChildrenIter<'a> {
    /// Create new iterator over branch node children.
    fn new(node: &BranchNodeRef<'a>) -> Self {
        Self {
            range: CHILD_INDEX_RANGE,
            state_mask: node.state_mask,
            stack_iter: node.stack[node.first_child_index()..].iter(),
        }
    }
}

impl<'a> Iterator for BranchChildrenIter<'a> {
    type Item = (u8, Option<&'a B256>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let i = self.range.next()?;
        let value = self
            .state_mask
            .is_bit_set(i)
            .then(|| unsafe { self.stack_iter.next().unwrap_unchecked() });
        Some((i, value))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
}

impl core::iter::FusedIterator for BranchChildrenIter<'_> {}

impl ExactSizeIterator for BranchChildrenIter<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.range.len()
    }
}
