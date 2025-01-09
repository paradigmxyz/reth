use reth_trie::Nibbles;

/// A type that can return its bytes representation encoded as a little-endian on 32 bytes.
pub(crate) trait AsBytes {
    /// Returns the type as its canonical little-endian representation on 32 bytes.
    fn as_bytes(&self) -> [u8; 32];
}

impl AsBytes for Nibbles {
    fn as_bytes(&self) -> [u8; 32] {
        // This is strange we are now representing the leaf key using big endian??
        let mut result = [0u8; 32];
        for (byte_index, bytes) in self.as_slice().chunks(8).enumerate() {
            for (bit_index, byte) in bytes.iter().enumerate() {
                result[byte_index] |= byte << bit_index;
            }
        }

        result
    }
}
