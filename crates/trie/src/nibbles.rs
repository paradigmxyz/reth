use derive_more::{Deref, DerefMut, From, Index};
use reth_rlp::RlpEncodableWrapper;

/// Structure representing a sequence of nibbles.
///
/// A nibble is a 4-bit value, and this structure is used to store
/// the nibble sequence representing the keys in a Merkle Patricia Trie (MPT).
/// Using nibbles simplifies trie operations and enables consistent key
/// representation in the MPT.
///
/// The `hex_data` field is a `Vec<u8>` that stores the nibbles, with each
/// `u8` value containing a single nibble. This means that each byte in
/// `hex_data` has its upper 4 bits set to zero and the lower 4 bits
/// representing the nibble value.
#[derive(
    Default,
    Clone,
    Eq,
    PartialEq,
    RlpEncodableWrapper,
    PartialOrd,
    Ord,
    Hash,
    Index,
    From,
    Deref,
    DerefMut,
)]
pub struct Nibbles {
    /// The inner representation of the nibble sequence.
    pub hex_data: Vec<u8>,
}

impl From<&[u8]> for Nibbles {
    fn from(slice: &[u8]) -> Self {
        Nibbles::from_hex(slice.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for Nibbles {
    fn from(arr: &[u8; N]) -> Self {
        Nibbles::from_hex(arr.to_vec())
    }
}

impl std::fmt::Debug for Nibbles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nibbles").field("hex_data", &hex::encode(&self.hex_data)).finish()
    }
}

impl Nibbles {
    /// Creates a new [Nibbles] instance from bytes.
    pub fn from_hex(hex: Vec<u8>) -> Self {
        Nibbles { hex_data: hex }
    }

    /// Take a byte array (slice or vector) as input and convert it into a [Nibbles] struct
    /// containing the nibbles (half-bytes or 4 bits) that make up the input byte data.
    pub fn unpack<T: AsRef<[u8]>>(data: T) -> Self {
        Nibbles { hex_data: data.as_ref().iter().flat_map(|item| [item / 16, item % 16]).collect() }
    }

    /// Packs the nibbles stored in the struct into a byte vector.
    ///
    /// This method combines each pair of consecutive nibbles into a single byte,
    /// effectively reducing the size of the data by a factor of two.
    /// If the number of nibbles is odd, the last nibble is shifted left by 4 bits and
    /// added to the packed byte vector.
    pub fn pack(&self) -> Vec<u8> {
        let length = (self.len() + 1) / 2;
        if length == 0 {
            Vec::new()
        } else {
            self.iter()
                .enumerate()
                .filter_map(|(index, nibble)| {
                    if index % 2 == 0 {
                        let next_nibble = self.get(index + 1).unwrap_or(&0);
                        Some((*nibble << 4) + *next_nibble)
                    } else {
                        None
                    }
                })
                .collect()
        }
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
    /// # use reth_trie::Nibbles;
    ///
    /// // Extension node with an even path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C, 0x0D]);
    /// assert_eq!(nibbles.encode_path_leaf(false), vec![0x00, 0xAB, 0xCD]);
    ///
    /// // Extension node with an odd path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C]);
    /// assert_eq!(nibbles.encode_path_leaf(false), vec![0x1A, 0xBC]);
    ///
    /// // Leaf node with an even path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C, 0x0D]);
    /// assert_eq!(nibbles.encode_path_leaf(true), vec![0x20, 0xAB, 0xCD]);
    ///
    /// // Leaf node with an odd path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C]);
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
    pub fn increment(&self) -> Option<Nibbles> {
        let mut incremented = self.hex_data.clone();

        for nibble in incremented.iter_mut().rev() {
            assert!(*nibble < 0x10);
            if *nibble < 0xf {
                *nibble += 1;
                return Some(Nibbles::from(incremented))
            } else {
                *nibble = 0;
            }
        }

        None
    }

    /// The last element of the hex vector is used to determine whether the nibble sequence
    /// represents a leaf or an extension node. If the last element is 0x10 (16), then it's a leaf.
    pub fn is_leaf(&self) -> bool {
        self.hex_data[self.hex_data.len() - 1] == 16
    }

    /// Returns `true` if the current nibble sequence starts with the given prefix.
    pub fn has_prefix(&self, other: &Self) -> bool {
        self.starts_with(other)
    }

    /// Returns the nibble at the given index.
    pub fn at(&self, i: usize) -> usize {
        self.hex_data[i] as usize
    }

    /// Returns the last nibble of the current nibble sequence.
    pub fn last(&self) -> Option<u8> {
        self.hex_data.last().copied()
    }

    /// Returns the length of the common prefix between the current nibble sequence and the given.
    pub fn common_prefix_length(&self, other: &Nibbles) -> usize {
        let len = std::cmp::min(self.len(), other.len());
        for i in 0..len {
            if self[i] != other[i] {
                return i
            }
        }
        len
    }

    /// Slice the current nibbles from the given start index to the end.
    pub fn slice_from(&self, index: usize) -> Nibbles {
        self.slice(index, self.hex_data.len())
    }

    /// Slice the current nibbles within the provided index range.
    pub fn slice(&self, start: usize, end: usize) -> Nibbles {
        Nibbles::from_hex(self.hex_data[start..end].to_vec())
    }

    /// Join two nibbles together.
    pub fn join(&self, b: &Nibbles) -> Nibbles {
        let mut hex_data = Vec::with_capacity(self.len() + b.len());
        hex_data.extend_from_slice(self);
        hex_data.extend_from_slice(b);
        Nibbles::from_hex(hex_data)
    }

    /// Extend the current nibbles with another nibbles.
    pub fn extend(&mut self, b: impl AsRef<[u8]>) {
        self.hex_data.extend_from_slice(b.as_ref());
    }

    /// Truncate the current nibbles to the given length.
    pub fn truncate(&mut self, len: usize) {
        self.hex_data.truncate(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn hashed_regression() {
        let nibbles = hex::decode("05010406040a040203030f010805020b050c04070003070e0909070f010b0a0805020301070c0a0902040b0f000f0006040a04050f020b090701000a0a040b").unwrap();
        let nibbles = Nibbles::from(nibbles);
        let path = nibbles.encode_path_leaf(true);
        let expected =
            hex::decode("351464a4233f1852b5c47037e997f1ba852317ca924bf0f064a45f2b9710aa4b")
                .unwrap();
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
            let nibbles = Nibbles::from(input);
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
