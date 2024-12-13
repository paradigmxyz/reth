use crate::Nibbles;
use alloy_primitives::{keccak256, B256};
use smallvec::SmallVec;

/// Trait for hashing keys in state.
pub trait KeyHasher: Default + Clone + Send + Sync + 'static {
    /// Hashes the given bytes into a 256-bit hash.
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256;
}

/// A key hasher that uses the Keccak-256 hash function.
#[derive(Clone, Debug, Default)]
pub struct KeccakKeyHasher;

impl KeyHasher for KeccakKeyHasher {
    fn hash_key<T: AsRef<[u8]>>(bytes: T) -> B256 {
        keccak256(bytes)
    }
}

/// The maximum number of bits a key can contain.
const MAX_BITS: usize = 254;

/// The maximum number of bytes a key can contain.
const MAX_BYTES: usize = 32;

// TODO(scroll): Refactor this into a trait that is more generic and can be used by any
// implementation that requires converting between nibbles and bits. Better yet we should use a
// trait that allows for defining the key type via a GAT as opposed to using Nibbles.

/// A trait for converting a `Nibbles` representation to a bit representation.
pub trait BitsCompatibility: Sized {
    /// Unpacks the bits from the provided bytes such that there is a byte for each bit in the
    /// input. The representation is big-endian with respect to the input.
    ///
    /// We truncate the Nibbles such that we only have 254 (bn254 field size) bits.
    fn unpack_bits<T: AsRef<[u8]>>(data: T) -> Self;

    /// Pack the bits into a byte representation.
    fn pack_bits(&self) -> SmallVec<[u8; 32]>;

    /// Encodes a leaf key represented as [`Nibbles`] into it's canonical little-endian
    /// representation.
    fn encode_leaf_key(&self) -> [u8; 32];

    /// Increment the key to the next key.
    fn increment_bit(&self) -> Option<Self>;
}

impl BitsCompatibility for Nibbles {
    fn unpack_bits<T: AsRef<[u8]>>(data: T) -> Self {
        let data = data.as_ref();
        let bits = data
            .iter()
            .take(MAX_BYTES)
            .flat_map(|&byte| (0..8).rev().map(move |i| (byte >> i) & 1))
            .take(MAX_BITS)
            .collect();

        Self::from_vec_unchecked(bits)
    }

    fn pack_bits(&self) -> SmallVec<[u8; 32]> {
        let mut result = SmallVec::with_capacity((self.len() + 7) / 8);

        for bits in self.as_slice().chunks(8) {
            let mut byte = 0;
            for (bit_index, bit) in bits.iter().enumerate() {
                byte |= *bit << (7 - bit_index);
            }
            result.push(byte);
        }

        result
    }

    fn encode_leaf_key(&self) -> [u8; 32] {
        // This is strange we are now representing the leaf key using big endian??
        let mut result = [0u8; 32];
        for (byte_index, bytes) in self.as_slice().chunks(8).enumerate() {
            for (bit_index, byte) in bytes.iter().enumerate() {
                result[byte_index] |= byte << bit_index;
            }
        }

        result
    }

    fn increment_bit(&self) -> Option<Self> {
        let mut incremented = self.clone();

        for nibble in incremented.as_mut_vec_unchecked().iter_mut().rev() {
            if *nibble < 1 {
                *nibble += 1;
                return Some(incremented);
            }

            *nibble = 0;
        }

        None
    }
}

/// Helper method to unpack into [`Nibbles`] from a byte slice.
///
/// For the `scroll` feature, this method will unpack the bits from the provided bytes such that
/// there is a byte for each bit in the input. The representation is big-endian with respect to the
/// input. When the `scroll` feature is not enabled, this method will unpack the bytes into nibbles.
pub fn unpack_nibbles<T: AsRef<[u8]>>(data: T) -> Nibbles {
    #[cfg(feature = "scroll")]
    let nibbles = Nibbles::unpack_bits(data);
    #[cfg(not(feature = "scroll"))]
    let nibbles = Nibbles::unpack(data);
    nibbles
}

/// Helper method to pack into a byte slice from [`Nibbles`].
///
/// For the `scroll` feature, this method will pack the bits into a byte representation. When the
/// `scroll` feature is not enabled, this method will pack the nibbles into bytes.
pub fn pack_nibbles(nibbles: &Nibbles) -> SmallVec<[u8; 32]> {
    #[cfg(feature = "scroll")]
    let packed = nibbles.pack_bits();
    #[cfg(not(feature = "scroll"))]
    let packed = nibbles.pack();
    packed
}
