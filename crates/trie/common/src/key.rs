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

/// Returns the key as-is when `use_hashed_state` is enabled (already hashed in changeset),
/// or applies keccak256 when disabled.
#[inline]
pub fn maybe_hash_key(key: B256, use_hashed_state: bool) -> B256 {
    if use_hashed_state {
        key
    } else {
        keccak256(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn test_maybe_hash_key_passthrough() {
        let key = B256::repeat_byte(0xab);
        assert_eq!(maybe_hash_key(key, true), key);
    }

    #[test]
    fn test_maybe_hash_key_hashes() {
        let key = B256::repeat_byte(0xab);
        assert_eq!(maybe_hash_key(key, false), keccak256(key));
    }

    #[test]
    fn test_maybe_hash_key_legacy_always_hashes() {
        let keys = [
            B256::ZERO,
            B256::from(U256::from(1)),
            B256::from(U256::from(0xdeadbeef_u64)),
            B256::repeat_byte(0xff),
        ];
        for key in keys {
            assert_eq!(maybe_hash_key(key, false), keccak256(key));
            assert_ne!(maybe_hash_key(key, false), key);
        }
    }

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
