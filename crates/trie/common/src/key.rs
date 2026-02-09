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

/// A key hasher that returns the input bytes as-is, assuming they are already a 32-byte hash.
#[derive(Clone, Debug, Default)]
pub struct IdentityKeyHasher;

impl KeyHasher for IdentityKeyHasher {
    #[inline]
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        B256::from_slice(bytes.as_ref())
    }
}
