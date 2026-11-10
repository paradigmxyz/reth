use alloc::vec::Vec;
use alloy_primitives::{keccak256, B256};

/// Trait for hashing keys in state.
pub trait KeyHasher: Default + Clone + Send + Sync + 'static {
    /// Hashes the given bytes into a 256-bit hash.
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256;

    /// Hashes every key, in order. Implementations may hash them together faster than one by one.
    fn hash_keys<T: AsRef<[u8]>>(keys: &[T]) -> Vec<B256> {
        keys.iter().map(Self::hash_key).collect()
    }
}

/// A key hasher that uses the Keccak-256 hash function.
#[derive(Clone, Debug, Default)]
pub struct KeccakKeyHasher;

impl KeyHasher for KeccakKeyHasher {
    #[inline]
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        keccak256(bytes)
    }

    #[inline]
    fn hash_keys<T: AsRef<[u8]>>(keys: &[T]) -> Vec<B256> {
        crate::keccak256_batch(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    #[test]
    fn test_keccak_key_hasher_always_hashes_regardless_of_length() {
        let addr = Address::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(addr), keccak256(addr));

        let slot = B256::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(slot), keccak256(slot));
        assert_ne!(KeccakKeyHasher::hash_key(slot), slot);
    }

    #[test]
    fn test_keccak_key_hasher_hash_keys_matches_hash_key() {
        let keys: Vec<B256> = (0..20u8).map(B256::repeat_byte).collect();
        let expected: Vec<B256> = keys.iter().map(KeccakKeyHasher::hash_key).collect();
        assert_eq!(KeccakKeyHasher::hash_keys(&keys), expected);
    }
}
