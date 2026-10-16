use alloy_primitives::{keccak256, keccak256_batch, B256};

/// Trait for hashing keys in state.
pub trait KeyHasher: Default + Clone + Send + Sync + 'static {
    /// Hashes the given bytes into a 256-bit hash.
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256;

    /// Hashes the key of each item and calls `f` with the item and the hash, in input order.
    ///
    /// Gives the same hashes as calling [`Self::hash_key`] on each key, but implementations may
    /// hash several keys at once.
    #[inline]
    fn hash_keys<I, K>(
        items: I,
        mut key: impl FnMut(&I::Item) -> K,
        mut f: impl FnMut(I::Item, B256),
    ) where
        I: IntoIterator,
        K: AsRef<[u8]>,
    {
        for item in items {
            let hash = Self::hash_key(key(&item));
            f(item, hash);
        }
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
    fn hash_keys<I, K>(
        items: I,
        mut key: impl FnMut(&I::Item) -> K,
        mut f: impl FnMut(I::Item, B256),
    ) where
        I: IntoIterator,
        K: AsRef<[u8]>,
    {
        let keyed = items.into_iter().map(|item| Keyed(key(&item), item));
        keccak256_batch(keyed, |Keyed(_, item), hash| f(item, hash));
    }
}

/// An item with the key it is hashed by.
struct Keyed<K, T>(K, T);

impl<K: AsRef<[u8]>, T> AsRef<[u8]> for Keyed<K, T> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn test_keccak_key_hasher_always_hashes_regardless_of_length() {
        use alloy_primitives::Address;

        let addr = Address::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(addr), keccak256(addr));

        let slot = B256::repeat_byte(0x42);
        assert_eq!(KeccakKeyHasher::hash_key(slot), keccak256(slot));
        assert_ne!(KeccakKeyHasher::hash_key(slot), slot);
    }

    #[test]
    fn test_keccak_key_hasher_hash_keys_matches_hash_key() {
        let items = (0..20u8).map(|i| (B256::repeat_byte(i), i)).collect::<Vec<_>>();
        let mut hashed = Vec::new();
        KeccakKeyHasher::hash_keys(
            &items,
            |(key, _)| *key,
            |item, hash| hashed.push((*item, hash)),
        );
        let expected = items
            .iter()
            .map(|&(key, value)| ((key, value), KeccakKeyHasher::hash_key(key)))
            .collect::<Vec<_>>();
        assert_eq!(hashed, expected);
    }
}
