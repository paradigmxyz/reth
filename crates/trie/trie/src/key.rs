use smallvec::SmallVec;

use crate::Nibbles;

/// The maximum number of bits a key can contain.
pub const MAX_BITS: usize = 248;

/// The maximum number of bytes a key can contain.
const MAX_BYTES: usize = 31;

// TODO(scroll): Refactor this into a trait that is more generic and can be used by any
// implementation that requires converting between nibbles and bits. Better yet we should use a
// trait that allows for defining the key type via a GAT as opposed to using Nibbles.

/// A trait for converting a `Nibbles` representation to a bit representation.
pub trait BitsCompatibility: Sized {
    /// Unpacks the bits from the provided bytes such that there is a byte for each bit in the
    /// input. The representation is big-endian with respect to the input.
    ///
    /// We truncate the Nibbles such that we only have [`MAX_BITS`] (248) bits.
    fn unpack_and_truncate_bits<T: AsRef<[u8]>>(data: T) -> Self;

    /// Pack the bits into a byte representation.
    fn pack_bits(&self) -> SmallVec<[u8; 32]>;

    /// Encodes a leaf key represented as [`Nibbles`] into it's canonical little-endian
    /// representation.
    fn encode_leaf_key(&self) -> [u8; 32];

    /// Increment the key to the next key.
    fn increment_bit(&self) -> Option<Self>;
}

impl BitsCompatibility for Nibbles {
    fn unpack_and_truncate_bits<T: AsRef<[u8]>>(data: T) -> Self {
        let data = data.as_ref();
        let unpacked_len = core::cmp::min(data.len() * 8, MAX_BITS);
        let mut bits = Vec::with_capacity(unpacked_len);

        for byte in data.iter().take(MAX_BYTES) {
            for i in (0..8).rev() {
                bits.push(byte >> i & 1);
            }
        }

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
