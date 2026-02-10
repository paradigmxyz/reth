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

/// A [`KeyHasher`] for `use_hashed_state` mode where storage slot keys in changesets
/// are already `keccak256`-hashed (32 bytes) but addresses remain plain (20 bytes).
///
/// 32-byte inputs are returned as-is; shorter inputs (addresses) are `keccak256`-hashed.
#[derive(Clone, Debug, Default)]
pub struct PreHashedKeyHasher;

impl KeyHasher for PreHashedKeyHasher {
    #[inline]
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        let b = bytes.as_ref();
        if b.len() == 32 {
            B256::from_slice(b)
        } else {
            keccak256(b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

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
    fn test_pre_hashed_key_hasher_32_bytes_passthrough() {
        let key = B256::repeat_byte(0xcd);
        assert_eq!(PreHashedKeyHasher::hash_key(key), key);
    }

    #[test]
    fn test_pre_hashed_key_hasher_20_bytes_hashes() {
        let addr = Address::repeat_byte(0x01);
        assert_eq!(PreHashedKeyHasher::hash_key(addr), KeccakKeyHasher::hash_key(addr),);
    }

    #[test]
    fn test_pre_hashed_key_hasher_other_lengths() {
        for input in [vec![], vec![0xff], vec![0xaa; 16], vec![0xbb; 31]] {
            assert_eq!(PreHashedKeyHasher::hash_key(&input), keccak256(&input));
        }
    }
}
