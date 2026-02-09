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

/// A key hasher that passes through 32-byte inputs as-is (already hashed storage keys)
/// and applies keccak256 to shorter inputs (e.g. 20-byte addresses).
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
