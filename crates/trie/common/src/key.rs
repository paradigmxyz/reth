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

/// A no-op key hasher that passes through already-hashed keys.
///
/// Use this when storage keys in changesets are already hashed (e.g., when `use_hashed_state` is
/// enabled). The input must be exactly 32 bytes representing a pre-hashed `B256` value.
#[derive(Clone, Debug, Default)]
pub struct IdentityKeyHasher;

impl KeyHasher for IdentityKeyHasher {
    #[inline]
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        B256::from_slice(bytes.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak_key_hasher() {
        let input = [0x42u8; 20];
        let result = KeccakKeyHasher::hash_key(&input);
        assert_eq!(result, keccak256(&input));
    }

    #[test]
    fn identity_key_hasher_passthrough() {
        let hash = B256::repeat_byte(0xab);
        let result = IdentityKeyHasher::hash_key(hash);
        assert_eq!(result, hash);
    }
}
