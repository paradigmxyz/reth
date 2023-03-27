use reth_rlp::RlpEncodableWrapper;
use std::{
    cmp::min,
    fmt::Debug,
    ops::{Index, Range, RangeFrom},
};

#[derive(Default, Clone, Eq, PartialEq, RlpEncodableWrapper, PartialOrd, Ord)]
/// Structure representing a sequence of nibbles.
///
/// A nibble is a 4-bit value, and this structure is used to store
/// the nibble sequence representing the keys in a Merkle Patricia Trie (MPT).
/// Using nibbles simplifies trie operations and enables consistent key
/// representation in the MPT.
///
/// The `hex_data` field is a `Vec<u8>` that stores the nibbles, with each
/// u8 value containing a single nibble. This means that each byte in
/// `hex_data` has its upper 4 bits set to zero and the lower 4 bits
/// representing the nibble value.
pub struct Nibbles {
    /// The inner representation of the nibble sequence.
    pub hex_data: Vec<u8>,
}

impl Debug for Nibbles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nibbles").field("hex_data", &hex::encode(&self.hex_data)).finish()
    }
}

impl Index<usize> for Nibbles {
    type Output = u8;

    fn index(&self, index: usize) -> &Self::Output {
        &self.hex_data[index]
    }
}

impl Index<RangeFrom<usize>> for Nibbles {
    type Output = [u8];

    fn index(&self, index: RangeFrom<usize>) -> &Self::Output {
        &self.hex_data[index]
    }
}

impl Index<Range<usize>> for Nibbles {
    type Output = [u8];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        &self.hex_data[index]
    }
}

impl From<Vec<u8>> for Nibbles {
    fn from(slice: Vec<u8>) -> Self {
        Nibbles::from_hex(slice)
    }
}

impl From<&[u8]> for Nibbles {
    fn from(slice: &[u8]) -> Self {
        Nibbles::from_hex(slice.to_vec())
    }
}

impl<const N: usize> From<[u8; N]> for Nibbles {
    fn from(arr: [u8; N]) -> Self {
        Nibbles::from_hex(arr.to_vec())
    }
}

impl Nibbles {
    /// Clears the nibble sequence.
    pub fn clear(&mut self) {
        self.hex_data.clear();
    }

    pub fn from_hex(hex: Vec<u8>) -> Self {
        Nibbles { hex_data: hex }
    }

    pub fn unpack<T: AsRef<[u8]>>(data: T) -> Self {
        let raw = data.as_ref();
        let mut hex_data = Vec::with_capacity(raw.len() * 2);
        for item in raw.into_iter() {
            hex_data.push(item / 16);
            hex_data.push(item % 16);
        }
        Nibbles { hex_data }
    }

    pub fn pack(&self) -> Vec<u8> {
        let nibbles = &self.hex_data;

        let n = (nibbles.len() + 1) / 2;
        let mut out = vec![0u8; n];
        if n == 0 {
            return out
        }

        let mut i = 0;
        let mut j = 0;
        while j < nibbles.len() {
            out[i] = nibbles[j] << 4;
            j += 1;
            if j < nibbles.len() {
                out[i] += nibbles[j];
                j += 1;
                i += 1;
            }
        }

        out
    }

    /// The compact encoding is a space-efficient way to represent a nibble sequence.
    ///
    /// In this format, the first byte (flag) encodes two pieces of information:
    /// 1. Whether the represented key corresponds to a leaf or an extension node in the trie.
    /// 2. Whether the nibble sequence has an odd or even number of nibbles.
    ///
    /// The flag is a single byte (u8), and the code checks the upper 4 bits of this byte to
    /// determine the type of node (leaf or extension) and the parity of the nibble count (odd
    /// or even).
    ///
    /// Here's a breakdown of the flags and their meanings:
    /// - 0x00: The key has an even number of nibbles and corresponds to an extension node.
    /// - 0x10: The key has an odd number of nibbles and corresponds to an extension node.
    /// - 0x20: The key has an even number of nibbles and corresponds to a leaf node.
    /// - 0x30: The key has an odd number of nibbles and corresponds to a leaf node.
    ///
    /// The code uses a match statement to handle each of these cases accordingly, updating the
    /// hex vector's prefix for 0x1 and 0x3, to declare it's an odd extension or leaf node.
    ///
    /// Finally, if it's a leaf node, it'll also push 0x0F (16) to the end of the hex vector.
    ///
    /// # Examples
    ///
    /// ```
    /// # use reth_provider::trie_v2::nibbles::Nibbles;
    /// // Extension node with an even path length:
    /// let compact = vec![0x00, 0xAB, 0xCD];
    /// let nibbles = Nibbles::decode_path(compact);
    /// assert_eq!(nibbles.hex_data, vec![0x0A, 0x0B, 0x0C, 0x0D]);
    ///
    /// // Extension node with an odd path length:
    /// let compact = vec![0x1A, 0xBC];
    /// let nibbles = Nibbles::decode_path(compact);
    /// assert_eq!(nibbles.hex_data, vec![0x0A, 0x0B, 0x0C]);
    ///
    /// // Leaf node with an even path length:
    /// let compact = vec![0x20, 0xAB, 0xCD];
    /// let nibbles = Nibbles::decode_path(compact);
    /// assert_eq!(nibbles.hex_data, vec![0x0A, 0x0B, 0x0C, 0x0D, 0x10]);
    ///
    /// // Leaf node with an odd path length:
    /// let compact = vec![0x3A, 0xBC];
    /// let nibbles = Nibbles::decode_path(compact);
    /// assert_eq!(nibbles.hex_data, vec![0x0A, 0x0B, 0x0C, 0x10]);
    /// ```
    pub fn decode_path<T: AsRef<[u8]>>(compact: T) -> Self {
        let compact = compact.as_ref();
        let mut hex = vec![];
        let flag = compact[0];
        let mut is_leaf = false;
        // The code is using `/ 16` and `% 16` operations to extract the upper and lower nibbles
        // from each byte in the compact encoding after the flag. Since a byte can store two
        // nibbles (each nibble is 4 bits), dividing a byte by 16 (which is equivalent to 2^4) will
        // give you the upper nibble, and taking the modulo of the byte with 16 will give you the
        // lower nibble.
        match flag >> 4 {
            0x0 => {}
            0x1 => hex.push(flag % 16),
            0x2 => is_leaf = true,
            0x3 => {
                is_leaf = true;
                hex.push(flag % 16);
            }
            _ => panic!("invalid data"),
        }

        for item in &compact[1..] {
            hex.push(item / 16);
            hex.push(item % 16);
        }
        if is_leaf {
            hex.push(16);
        }

        Nibbles { hex_data: hex }
    }

    /// The last element of the hex vector is used to determine whether the nibble sequence
    /// represents a leaf or an extension node. If the last element is 0xF (16), then it's a leaf.
    pub fn is_leaf(&self) -> bool {
        self.hex_data[self.hex_data.len() - 1] == 16
    }

    /// Encodes the nibble sequence in the `hex_data` field into a compact byte representation
    /// with a header byte, following the Ethereum Merkle Patricia Trie compact encoding format.
    ///
    /// The method creates a header byte based on the following rules:
    /// - If the node is an extension and the path length is even, the header byte is 0x00.
    /// - If the node is an extension and the path length is odd, the header byte is 0x10 + first
    ///   hex char.
    /// - If the node is a leaf and the path length is even, the header byte is 0x20.
    /// - If the node is a leaf and the path length is odd, the header byte is 0x30 + first hex
    ///   char.
    ///
    /// After the header byte is determined, the method merges the nibble pairs back into bytes,
    /// similar to the `encode_raw` method, and appends them to the compact byte sequence.
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the compact byte representation of the nibble sequence, including the
    /// header byte.
    ///
    /// # Example
    ///
    /// ```
    /// # use reth_provider::trie_v2::nibbles::Nibbles;
    ///
    /// // Extension node with an even path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C, 0x0D]);
    /// let compact = nibbles.encode_path_leaf();
    /// assert_eq!(compact, vec![0x00, 0xAB, 0xCD]);
    ///
    /// // Extension node with an odd path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C]);
    /// let compact = nibbles.encode_path_leaf();
    /// assert_eq!(compact, vec![0x1A, 0xBC]);
    ///
    /// // Leaf node with an even path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C, 0x0D, 0x10]);
    /// let compact = nibbles.encode_path_leaf();
    /// assert_eq!(compact, vec![0x20, 0xAB, 0xCD]);
    ///
    /// // Leaf node with an odd path length:
    /// let nibbles = Nibbles::from_hex(vec![0x0A, 0x0B, 0x0C, 0x10]);
    /// let compact = nibbles.encode_path_leaf();
    /// assert_eq!(compact, vec![0x3A, 0xBC]);
    /// ```
    pub fn encode_path(&self) -> Vec<u8> {
        let is_leaf = self.is_leaf();
        self.encode_path_leaf(is_leaf)
    }

    pub fn encode_path_leaf(&self, is_leaf: bool) -> Vec<u8> {
        // Handle leaf being empty edge case
        if is_leaf && self.is_empty() {
            return vec![0x20]
        } else if self.is_empty() {
            return vec![0x00]
        }

        let mut compact = Vec::with_capacity(self.len() / 2 + 1);
        let is_odd = self.len() % 2 == 1;
        // node type    path length    |    prefix    hexchar
        // --------------------------------------------------
        // extension    even           |    0000      0x0
        // extension    odd            |    0001      0x1
        // leaf         even           |    0010      0x2
        // leaf         odd            |    0011      0x3
        compact.push(if is_leaf { 0x20 } else { 0x00 });
        let mut hex = self.hex_data.as_slice();

        if self.len() == 1 {
            compact[0] += hex[0];
            if is_odd {
                compact[0] += 0x10;
            }
            return compact
        }

        if is_odd {
            compact[0] += 0x10;
            hex = &hex[1..];
        }

        for i in 0..(hex.len() / 2) {
            compact.push((hex[i * 2] * 16) + (hex[i * 2 + 1]));
        }

        compact
    }

    pub fn len(&self) -> usize {
        self.hex_data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn at(&self, i: usize) -> usize {
        self.hex_data[i] as usize
    }

    pub fn prefix_length(&self, other: &Nibbles) -> usize {
        let len = std::cmp::min(self.len(), other.len());
        for i in 0..len {
            if self[i] != other[i] {
                return i
            }
        }
        len
    }

    // Same as prefix_length:w
    pub fn common_prefix(&self, other_partial: &Nibbles) -> usize {
        let s = min(self.len(), other_partial.len());
        let mut i = 0usize;
        while i < s {
            if self.at(i) != other_partial.at(i) {
                break
            }
            i += 1;
        }
        i
    }

    pub fn offset(&self, index: usize) -> Nibbles {
        self.slice(index, self.hex_data.len())
    }

    pub fn slice(&self, start: usize, end: usize) -> Nibbles {
        Nibbles::from_hex(self.hex_data[start..end].to_vec())
    }

    pub fn get_data(&self) -> &[u8] {
        &self.hex_data
    }

    pub fn join(&self, b: &Nibbles) -> Nibbles {
        let mut hex_data = vec![];
        hex_data.extend_from_slice(self.get_data());
        hex_data.extend_from_slice(b.get_data());
        Nibbles::from_hex(hex_data)
    }

    pub fn extend(&mut self, b: &Nibbles) {
        self.hex_data.extend_from_slice(b.get_data());
    }

    pub fn truncate(&mut self, len: usize) {
        self.hex_data.truncate(len)
    }

    pub fn pop(&mut self) -> Option<u8> {
        self.hex_data.pop()
    }

    pub fn push(&mut self, e: u8) {
        self.hex_data.push(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_nibbles() {
        for (input, expected) in [
            (vec![], vec![0]),
            (vec![0xa], vec![0x1a]),
            (vec![0xa, 0xb], vec![0x00, 0xab]),
            (vec![0xa, 0xb, 0x2], vec![0xab, 0x20]),
            (vec![0xa, 0xb, 0x2, 0x0], vec![0xab, 0x20]),
            (vec![0xa, 0xb, 0x2, 0x7], vec![0xab, 0x27]),
        ] {
            let nibbles = Nibbles::from(input);
            let encoded = nibbles.pack();
            assert_eq!(encoded, expected);
        }
    }

    #[test]
    // TODO: Covnert to proptest
    fn pack_unpack_roundtrip() {
        let input = vec![0xab, 0x27];
        let nibbles = Nibbles::unpack(&input);
        let packed = nibbles.pack();
        assert_eq!(packed, input);
    }

    #[test]
    // TODO: Covnert to proptest
    fn path_even_leaf_roundtrip() {
        let input = Nibbles::unpack(vec![0xab, 0x27]);
        let compact = input.encode_path_leaf(false);
        let decoded = Nibbles::decode_path(compact);
        assert_eq!(decoded, input);
    }

    #[test]
    // TODO: Covnert to proptest
    fn path_odd_leaf_roundtrip() {
        let input = Nibbles::unpack(vec![0xab, 0x27, 0xc]);
        let compact = input.encode_path_leaf(false);
        let decoded = Nibbles::decode_path(compact);
        assert_eq!(decoded, input);
    }
}
