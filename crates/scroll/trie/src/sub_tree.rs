use super::{BRANCH_NODE_LBRT_DOMAIN, BRANCH_NODE_LTRB_DOMAIN};
use alloy_primitives::{hex, B256};
use alloy_trie::Nibbles;
use core::fmt;
use reth_scroll_primitives::poseidon::{hash_with_domain, Fr, PrimeField};

/// [`SubTreeRef`] is a structure that allows for calculation of the root of a sparse binary Merkle
/// tree consisting of a single leaf node.
pub(crate) struct SubTreeRef<'a> {
    /// The key to the child node.
    pub key: &'a Nibbles,
    /// A pointer to the child node.
    pub child: &'a B256,
}

impl<'a> SubTreeRef<'a> {
    /// Creates a new subtree with the given key and a pointer to the child.
    #[inline]
    pub(crate) const fn new(key: &'a Nibbles, child: &'a B256) -> Self {
        Self { key, child }
    }

    pub(crate) fn root(&self) -> B256 {
        let mut tree_root =
            Fr::from_repr_vartime(self.child.0).expect("child is a valid field element");
        for bit in self.key.as_slice().iter().rev() {
            tree_root = if *bit == 0 {
                hash_with_domain(&[tree_root, Fr::zero()], BRANCH_NODE_LBRT_DOMAIN)
            } else {
                hash_with_domain(&[Fr::zero(), tree_root], BRANCH_NODE_LTRB_DOMAIN)
            };
        }
        tree_root.to_repr().into()
    }
}

impl fmt::Debug for SubTreeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubTreeRef")
            .field("key", &self.key)
            .field("node", &hex::encode(self.child))
            .finish()
    }
}
