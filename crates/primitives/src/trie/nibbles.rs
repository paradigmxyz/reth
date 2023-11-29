use crate::Bytes;
use alloy_rlp::RlpEncodableWrapper;
use derive_more::{Deref, From, Index};
use reth_codecs::{main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::ops::RangeBounds;

/// The nibbles are the keys for the AccountsTrie and the subkeys for the StorageTrie.
#[main_codec]
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
pub struct StoredNibbles(pub Bytes);

impl<T: Into<Nibbles>> From<T> for StoredNibbles {
    #[inline]
    fn from(value: T) -> Self {
        Self(value.into().into_bytes())
    }
}

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash, Deref)]
pub struct StoredNibblesSubKey(pub StoredNibbles);

impl<T: Into<StoredNibbles>> From<T> for StoredNibblesSubKey {
    #[inline]
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

impl Compact for StoredNibblesSubKey {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        // right-pad with zeros
        buf.put_slice(&self.0[..]);
        static ZERO: &[u8; 64] = &[0; 64];
        buf.put_slice(&ZERO[self.0.len()..]);

        buf.put_u8(self.0.len() as u8);
        64 + 1
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let len = buf[64] as usize;
        (Self(buf[..len].to_vec().into()), &buf[65..])
    }
}

/// Structure representing a sequence of nibbles.
///
/// A nibble is a 4-bit value, and this structure is used to store the nibble sequence representing
/// the keys in a Merkle Patricia Trie (MPT).
/// Using nibbles simplifies trie operations and enables consistent key representation in the MPT.
///
/// The internal representation is a shared heap-allocated vector ([`Bytes`]) that stores one nibble
/// per byte. This means that each byte has its upper 4 bits set to zero and the lower 4 bits
/// representing the nibble value.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    RlpEncodableWrapper,
    Index,
    From,
    Deref,
)]
pub struct Nibbles(Bytes);

impl From<&[u8]> for Nibbles {
    #[inline]
    fn from(value: &[u8]) -> Self {
        Self::from_nibbles(value.to_vec())
    }
}

impl<const N: usize> From<[u8; N]> for Nibbles {
    #[inline]
    fn from(value: [u8; N]) -> Self {
        Self::from_nibbles(value.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for Nibbles {
    #[inline]
    fn from(value: &[u8; N]) -> Self {
        Self::from_nibbles(value.to_vec())
    }
}

impl From<Vec<u8>> for Nibbles {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self::from_nibbles(value)
    }
}

impl Nibbles {
    /// Creates a new [`Nibbles`] instance from a nibble slice.
    #[inline]
    pub fn from_nibbles<T: Into<Bytes>>(nibbles: T) -> Self {
        Self(nibbles.into())
    }

    /// Converts a byte slice into a [`Nibbles`] instance containing the nibbles (half-bytes or 4
    /// bits) that make up the input byte data.
    pub fn unpack<T: AsRef<[u8]>>(data: T) -> Self {
        let data = data.as_ref();
        let mut nibbles = Vec::with_capacity(data.len() * 2);
        for &byte in data {
            nibbles.push(byte >> 4);
            nibbles.push(byte & 0x0f);
        }
        Self(nibbles.into())
    }

    /// Packs the nibbles stored in the struct into a byte vector.
    ///
    /// This method combines each pair of consecutive nibbles into a single byte,
    /// effectively reducing the size of the data by a factor of two.
    /// If the number of nibbles is odd, the last nibble is shifted left by 4 bits and
    /// added to the packed byte vector.
    pub fn pack(&self) -> Vec<u8> {
        let packed_len = (self.len() + 1) / 2;
        let mut v = Vec::with_capacity(packed_len);
        for i in 0..packed_len {
            let hi = *unsafe { self.get_unchecked(i * 2) };
            let lo = self.get(i * 2 + 1).copied().unwrap_or(0);
            v.push((hi << 4) | lo);
        }
        v
    }

    /// Encodes a given path leaf as a compact array of bytes, where each byte represents two
    /// "nibbles" (half-bytes or 4 bits) of the original hex data, along with additional information
    /// about the leaf itself.
    ///
    /// The method takes the following input:
    /// `is_leaf`: A boolean value indicating whether the current node is a leaf node or not.
    ///
    /// The first byte of the encoded vector is set based on the `is_leaf` flag and the parity of
    /// the hex data length (even or odd number of nibbles).
    ///  - If the node is an extension with even length, the header byte is `0x00`.
    ///  - If the node is an extension with odd length, the header byte is `0x10 + <first nibble>`.
    ///  - If the node is a leaf with even length, the header byte is `0x20`.
    ///  - If the node is a leaf with odd length, the header byte is `0x30 + <first nibble>`.
    ///
    /// If there is an odd number of nibbles, store the first nibble in the lower 4 bits of the
    /// first byte of encoded.
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the compact byte representation of the nibble sequence, including the
    /// header byte.
    ///
    /// # Example
    ///
    /// ```
    /// # use reth_primitives::trie::Nibbles;
    ///
    /// // Extension node with an even path length:
    /// let nibbles = Nibbles::from_nibbles([0x0A, 0x0B, 0x0C, 0x0D]);
    /// assert_eq!(nibbles.encode_path_leaf(false), vec![0x00, 0xAB, 0xCD]);
    ///
    /// // Extension node with an odd path length:
    /// let nibbles = Nibbles::from_nibbles([0x0A, 0x0B, 0x0C]);
    /// assert_eq!(nibbles.encode_path_leaf(false), vec![0x1A, 0xBC]);
    ///
    /// // Leaf node with an even path length:
    /// let nibbles = Nibbles::from_nibbles([0x0A, 0x0B, 0x0C, 0x0D]);
    /// assert_eq!(nibbles.encode_path_leaf(true), vec![0x20, 0xAB, 0xCD]);
    ///
    /// // Leaf node with an odd path length:
    /// let nibbles = Nibbles::from_nibbles([0x0A, 0x0B, 0x0C]);
    /// assert_eq!(nibbles.encode_path_leaf(true), vec![0x3A, 0xBC]);
    /// ```
    pub fn encode_path_leaf(&self, is_leaf: bool) -> Vec<u8> {
        let mut encoded = vec![0u8; self.len() / 2 + 1];
        let odd_nibbles = self.len() % 2 != 0;

        // Set the first byte of the encoded vector.
        encoded[0] = match (is_leaf, odd_nibbles) {
            (true, true) => 0x30 | self[0],
            (true, false) => 0x20,
            (false, true) => 0x10 | self[0],
            (false, false) => 0x00,
        };

        let mut nibble_idx = if odd_nibbles { 1 } else { 0 };
        for byte in encoded.iter_mut().skip(1) {
            *byte = (self[nibble_idx] << 4) + self[nibble_idx + 1];
            nibble_idx += 2;
        }

        encoded
    }

    /// Increments the nibble sequence by one.
    pub fn increment(&self) -> Option<Self> {
        let mut incremented = self.0.to_vec();

        for nibble in incremented.iter_mut().rev() {
            debug_assert!(*nibble < 0x10);
            if *nibble < 0xf {
                *nibble += 1;
                return Some(Self::from_nibbles(incremented))
            } else {
                *nibble = 0;
            }
        }

        None
    }

    /// The last element of the hex vector is used to determine whether the nibble sequence
    /// represents a leaf or an extension node. If the last element is 0x10 (16), then it's a leaf.
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.last() == Some(16)
    }

    /// Returns `true` if the current nibble sequence starts with the given prefix.
    #[inline]
    pub fn has_prefix(&self, other: &Self) -> bool {
        self.starts_with(other)
    }

    /// Returns the nibble at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    #[inline]
    #[track_caller]
    pub fn at(&self, i: usize) -> usize {
        self.0[i] as usize
    }

    /// Returns the last nibble of the current nibble sequence.
    #[inline]
    pub fn last(&self) -> Option<u8> {
        self.0.last().copied()
    }

    /// Returns the length of the common prefix between the current nibble sequence and the given.
    #[inline]
    pub fn common_prefix_length(&self, other: &Self) -> usize {
        let len = std::cmp::min(self.len(), other.len());
        for i in 0..len {
            if self[i] != other[i] {
                return i
            }
        }
        len
    }

    /// Returns a reference to the underlying [`Bytes`].
    #[inline]
    pub fn as_bytes(&self) -> &Bytes {
        &self.0
    }

    /// Returns the nibbles as a byte slice.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Returns the underlying [`Bytes`].
    #[inline]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// Slice the current nibbles within the provided index range.
    #[inline]
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        Self(self.0.slice(range))
    }

    /// Join two nibbles together.
    #[inline]
    pub fn join(&self, b: &Self) -> Self {
        let mut hex_data = Vec::with_capacity(self.len() + b.len());
        hex_data.extend_from_slice(self);
        hex_data.extend_from_slice(b);
        Self::from_nibbles(hex_data)
    }

    /// Pushes a nibble to the end of the current nibbles.
    ///
    /// **Note**: This method re-allocates on each call.
    #[inline]
    pub fn push(&mut self, nibble: u8) {
        self.extend([nibble]);
    }

    /// Extend the current nibbles with another nibbles.
    ///
    /// **Note**: This method re-allocates on each call.
    #[inline]
    pub fn extend(&mut self, b: impl AsRef<[u8]>) {
        let mut bytes = self.0.to_vec();
        bytes.extend_from_slice(b.as_ref());
        self.0 = bytes.into();
    }

    /// Truncates the current nibbles to the given length.
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    /// Clears the current nibbles.
    #[inline]
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;
    use proptest::prelude::*;

    #[test]
    fn hashed_regression() {
        let nibbles = Nibbles::from_nibbles(hex!("05010406040a040203030f010805020b050c04070003070e0909070f010b0a0805020301070c0a0902040b0f000f0006040a04050f020b090701000a0a040b"));
        let path = nibbles.encode_path_leaf(true);
        let expected = hex!("351464a4233f1852b5c47037e997f1ba852317ca924bf0f064a45f2b9710aa4b");
        assert_eq!(path, expected);
    }

    #[test]
    fn pack_nibbles() {
        for (input, expected) in [
            (vec![], vec![]),
            (vec![0xa], vec![0xa0]),
            (vec![0xa, 0xb], vec![0xab]),
            (vec![0xa, 0xb, 0x2], vec![0xab, 0x20]),
            (vec![0xa, 0xb, 0x2, 0x0], vec![0xab, 0x20]),
            (vec![0xa, 0xb, 0x2, 0x7], vec![0xab, 0x27]),
        ] {
            let nibbles = Nibbles::from_nibbles(input);
            let encoded = nibbles.pack();
            assert_eq!(encoded, expected);
        }
    }

    proptest! {
        #[test]
        fn pack_unpack_roundtrip(input in any::<Vec<u8>>()) {
            let nibbles = Nibbles::unpack(&input);
            let packed = nibbles.pack();
            prop_assert_eq!(packed, input);
        }

        #[test]
        fn encode_path_first_byte(input in any::<Vec<u8>>()) {
            prop_assume!(!input.is_empty());
            let input = Nibbles::unpack(input);
            let input_is_odd = input.len() % 2 == 1;

            let compact_leaf = input.encode_path_leaf(true);
            let leaf_flag = compact_leaf[0];
            // Check flag
            assert_ne!(leaf_flag & 0x20, 0);
            assert_eq!(input_is_odd, (leaf_flag & 0x10) != 0);
            if input_is_odd {
                assert_eq!(leaf_flag & 0x0f, *input.first().unwrap());
            }


            let compact_extension = input.encode_path_leaf(false);
            let extension_flag = compact_extension[0];
            // Check first byte
            assert_eq!(extension_flag & 0x20, 0);
            assert_eq!(input_is_odd, (extension_flag & 0x10) != 0);
            if input_is_odd {
                assert_eq!(extension_flag & 0x0f, *input.first().unwrap());
            }
        }
    }
}
