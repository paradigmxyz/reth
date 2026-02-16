use alloy_primitives::{keccak256, B256};

/// Trait for hashing keys in state.
pub trait KeyHasher: Default + Clone + Send + Sync + 'static {
    /// Hashes the given bytes into a 256-bit hash.
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256;
}

/// A key hasher that uses the Keccak-256 hash function.
#[derive(Clone, Debug, Default)]
pub struct KeccakKeyHasher;

impl KeyHasher for KeccakKeyHasher {
    #[inline]
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        keccak256(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keccak_key_hasher_always_hashes_regardless_of_length() {
        use alloy_primitives::Address;

        let addr = Address::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(addr), keccak256(addr));

        let slot = B256::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(slot), keccak256(slot));
        assert_ne!(KeccakKeyHasher::hash_key(slot), slot);
    }
}
