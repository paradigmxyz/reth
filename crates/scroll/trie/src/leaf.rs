use super::LEAF_NODE_DOMAIN;
use crate::key::AsBytes;
use alloy_primitives::B256;
use alloy_trie::nodes::LeafNodeRef;
use poseidon_bn254::{hash_with_domain, Fr, PrimeField};

/// A trait used to hash the leaf node.
pub(crate) trait HashLeaf {
    /// Hash the leaf node.
    fn hash_leaf(&self) -> B256;
}

impl HashLeaf for LeafNodeRef<'_> {
    fn hash_leaf(&self) -> B256 {
        let leaf_key =
            Fr::from_repr_vartime(self.key.as_bytes()).expect("leaf key is a valid field element");
        let leaf_value = Fr::from_repr_vartime(
            <[u8; 32]>::try_from(self.value).expect("leaf value is 32 bytes"),
        )
        .expect("leaf value is a valid field element");
        hash_with_domain(&[leaf_key, leaf_value], LEAF_NODE_DOMAIN).to_repr().into()
    }
}
