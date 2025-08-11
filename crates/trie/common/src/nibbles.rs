use alloc::vec::Vec;
use derive_more::Deref;
pub use nybbles::Nibbles;

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, derive_more::Index)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct StoredNibbles(pub Nibbles);

impl From<Nibbles> for StoredNibbles {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibbles {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibbles {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(&self.0.to_vec());
        self.0.len()
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        let nibbles = &buf[..len];
        buf.advance(len);
        (Self(Nibbles::from_nibbles_unchecked(nibbles)), buf)
    }
}

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "test-utils", derive(arbitrary::Arbitrary))]
pub struct StoredNibblesSubKey(pub Nibbles);

impl From<Nibbles> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl From<StoredNibblesSubKey> for Nibbles {
    #[inline]
    fn from(value: StoredNibblesSubKey) -> Self {
        value.0
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StoredNibblesSubKey {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        // New format: 
        // - 1 byte for length (number of nibbles)
        // - Packed nibbles (2 nibbles per byte)
        
        let nibbles_len = self.len();
        buf.put_u8(nibbles_len as u8);
        
        // Pack the nibbles - each pair of nibbles becomes one byte
        let packed_len = (nibbles_len + 1) / 2;
        
        // Use pack method from Nibbles to convert to packed bytes
        let packed = self.0.pack();
        for byte in packed.iter().take(packed_len) {
            buf.put_u8(*byte);
        }
        
        1 + packed_len
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        // Check if this is the old format or new format
        // Old format: 65 bytes total (64 bytes of nibbles + 1 byte length at position 64)
        // New format: 1 byte length at start + packed nibbles
        
        // We can detect the old format by checking if we have at least 65 bytes
        // and the byte at position 64 is a reasonable length (0-64)
        if buf.len() >= 65 {
            let potential_old_len = buf[64] as usize;
            if potential_old_len <= 64 {
                // Check if bytes after the length in old format are consistent
                // In old format, we expect exactly 65 bytes used
                // Try to parse as old format first
                let nibbles = &buf[..potential_old_len];
                
                // Verify this looks like old format - all values should be < 16
                let is_old_format = nibbles.iter().all(|&n| n < 16);
                
                if is_old_format {
                    // Old format detected
                    return (Self(Nibbles::from_nibbles_unchecked(nibbles)), &buf[65..]);
                }
            }
        }
        
        // New format: length byte followed by packed nibbles
        let nibbles_len = buf[0] as usize;
        let packed_len = (nibbles_len + 1) / 2;
        
        // Unpack the nibbles
        let mut nibbles = Vec::with_capacity(nibbles_len);
        let packed_data = &buf[1..1 + packed_len];
        
        for i in 0..nibbles_len / 2 {
            let byte = packed_data[i];
            nibbles.push(byte >> 4);
            nibbles.push(byte & 0x0F);
        }
        
        // Handle odd number of nibbles
        if nibbles_len % 2 == 1 {
            let byte = packed_data[packed_len - 1];
            nibbles.push(byte >> 4);
        }
        
        (Self(Nibbles::from_nibbles_unchecked(nibbles)), &buf[1 + packed_len..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use reth_codecs::Compact;

    #[test]
    fn test_stored_nibbles_from_nibbles() {
        let nibbles = Nibbles::from_nibbles_unchecked(vec![0x02, 0x04, 0x06]);
        let stored = StoredNibbles::from(nibbles);
        assert_eq!(stored.0, nibbles);
    }

    #[test]
    fn test_stored_nibbles_from_vec() {
        let bytes = vec![0x02, 0x04, 0x06];
        let stored = StoredNibbles::from(bytes.clone());
        assert_eq!(stored.0.to_vec(), bytes);
    }

    #[test]
    fn test_stored_nibbles_to_compact() {
        let stored = StoredNibbles::from(vec![0x02, 0x04]);
        let mut buf = BytesMut::with_capacity(10);
        let len = stored.to_compact(&mut buf);
        assert_eq!(len, 2);
        assert_eq!(buf, &vec![0x02, 0x04][..]);
    }

    #[test]
    fn test_stored_nibbles_from_compact() {
        let buf = vec![0x02, 0x04, 0x06];
        let (stored, remaining) = StoredNibbles::from_compact(&buf, 2);
        assert_eq!(stored.0.to_vec(), vec![0x02, 0x04]);
        assert_eq!(remaining, &[0x06]);
    }

    #[test]
    fn test_stored_nibbles_subkey_new_format_even_nibbles() {
        // Test new packed format with even number of nibbles
        let subkey = StoredNibblesSubKey::from(vec![0x02, 0x04, 0x06, 0x08]);
        let mut buf = BytesMut::with_capacity(10);
        let len = subkey.to_compact(&mut buf);
        
        // Should be 1 byte length + 2 bytes packed nibbles
        assert_eq!(len, 3);
        assert_eq!(buf[0], 4); // Length = 4 nibbles
        assert_eq!(buf[1], 0x24); // 0x02 << 4 | 0x04
        assert_eq!(buf[2], 0x68); // 0x06 << 4 | 0x08
    }
    
    #[test]
    fn test_stored_nibbles_subkey_new_format_odd_nibbles() {
        // Test new packed format with odd number of nibbles
        let subkey = StoredNibblesSubKey::from(vec![0x0A, 0x0B, 0x0C]);
        let mut buf = BytesMut::with_capacity(10);
        let len = subkey.to_compact(&mut buf);
        
        // Should be 1 byte length + 2 bytes packed nibbles
        assert_eq!(len, 3);
        assert_eq!(buf[0], 3); // Length = 3 nibbles
        assert_eq!(buf[1], 0xAB); // 0x0A << 4 | 0x0B
        assert_eq!(buf[2], 0xC0); // 0x0C << 4 | 0x00 (padding)
    }
    
    #[test]
    fn test_stored_nibbles_subkey_from_compact_new_format() {
        // Test decoding new format
        let buf = vec![
            4,     // length = 4 nibbles
            0x24,  // packed: 0x02, 0x04
            0x68,  // packed: 0x06, 0x08
            0xFF,  // extra data to verify we return correct remaining
        ];
        let (subkey, remaining) = StoredNibblesSubKey::from_compact(&buf, 0);
        assert_eq!(subkey.0.to_vec(), vec![0x02, 0x04, 0x06, 0x08]);
        assert_eq!(remaining, &[0xFF]);
    }
    
    #[test]
    fn test_stored_nibbles_subkey_from_compact_old_format() {
        // Test backwards compatibility with old format
        let mut buf = vec![0x02, 0x04, 0x06]; // First 3 nibbles
        buf.resize(64, 0); // Pad to 64 bytes
        buf.push(3); // Length byte at position 64
        buf.push(0xFF); // Extra data
        
        let (subkey, remaining) = StoredNibblesSubKey::from_compact(&buf, 0);
        assert_eq!(subkey.0.to_vec(), vec![0x02, 0x04, 0x06]);
        assert_eq!(remaining, &[0xFF]);
    }
    
    #[test]
    fn test_stored_nibbles_subkey_roundtrip() {
        // Test various lengths roundtrip correctly
        for len in [0, 1, 2, 3, 4, 16, 31, 32, 63, 64] {
            let nibbles: Vec<u8> = (0..len).map(|i| (i % 16) as u8).collect();
            let subkey = StoredNibblesSubKey::from(nibbles.clone());
            
            let mut buf = BytesMut::with_capacity(100);
            let encoded_len = subkey.to_compact(&mut buf);
            
            let (decoded, remaining) = StoredNibblesSubKey::from_compact(&buf, 0);
            assert_eq!(decoded.0.to_vec(), nibbles, "Failed for length {}", len);
            assert_eq!(remaining.len(), 0);
            
            // Verify space savings
            let old_size = 65; // Old format always uses 65 bytes
            let new_size = encoded_len;
            assert!(new_size <= old_size, "New format should not be larger");
            
            // For most cases, new format should be much smaller
            if len > 0 {
                let expected_new_size = 1 + (len + 1) / 2;
                assert_eq!(new_size, expected_new_size);
            }
        }
    }

    #[test]
    fn test_serialization_stored_nibbles() {
        let stored = StoredNibbles::from(vec![0x02, 0x04]);
        let serialized = serde_json::to_string(&stored).unwrap();
        let deserialized: StoredNibbles = serde_json::from_str(&serialized).unwrap();
        assert_eq!(stored, deserialized);
    }

    #[test]
    fn test_serialization_stored_nibbles_subkey() {
        let subkey = StoredNibblesSubKey::from(vec![0x02, 0x04]);
        let serialized = serde_json::to_string(&subkey).unwrap();
        let deserialized: StoredNibblesSubKey = serde_json::from_str(&serialized).unwrap();
        assert_eq!(subkey, deserialized);
    }
}
