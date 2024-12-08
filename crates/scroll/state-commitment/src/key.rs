use alloy_primitives::B256;
use reth_scroll_primitives::poseidon::{
    split_and_hash_be_bytes, PrimeField, FIELD_ELEMENT_REPR_BYTES,
};
use reth_trie::KeyHasher;

/// An implementation of a key hasher that uses Poseidon.
#[derive(Clone, Debug, Default)]
pub struct PoseidonKeyHasher;

impl KeyHasher for PoseidonKeyHasher {
    /// Hashes the key using the Poseidon hash function.
    ///
    /// The bytes are expected to be provided in big endian format.
    ///
    /// Panics if the number of bytes provided is greater than the number of bytes in the
    /// binary representation of a field element (32).
    ///
    /// Returns the hash digest in little endian representation with bits reversed.
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        debug_assert!(bytes.as_ref().len() <= FIELD_ELEMENT_REPR_BYTES);
        let mut bytes = split_and_hash_be_bytes(bytes.as_ref()).to_repr();
        bytes.iter_mut().for_each(|byte| *byte = byte.reverse_bits());
        bytes.into()
    }
}
