use super::{BranchNodeCompact, StoredNibblesSubKey};

/// Account storage trie node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieEntry {
    /// The nibbles of the intermediate node
    pub nibbles: StoredNibblesSubKey,
    /// Encoded node.
    pub node: BranchNodeCompact,
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageTrieEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.to_compact(buf);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        // Determine how many bytes the nibbles consumed
        let initial_buf_len = buf.len();
        let (nibbles, buf_after_nibbles) = StoredNibblesSubKey::from_compact(buf, 0);
        let nibbles_bytes_consumed = initial_buf_len - buf_after_nibbles.len();
        
        // The remaining bytes are for the BranchNodeCompact
        let node_len = len - nibbles_bytes_consumed;
        let (node, buf) = BranchNodeCompact::from_compact(buf_after_nibbles, node_len);
        let this = Self { nibbles, node };
        (this, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_codecs::Compact;
    
    #[test]
    fn test_storage_trie_entry_roundtrip() {
        // Create a test entry with some nibbles and a branch node
        let nibbles = StoredNibblesSubKey::from(vec![0x01, 0x02, 0x03, 0x04]);
        let node = BranchNodeCompact::default();
        
        let entry = StorageTrieEntry { nibbles: nibbles.clone(), node: node.clone() };
        
        // Encode it
        let mut buf = Vec::new();
        let encoded_len = entry.to_compact(&mut buf);
        
        println!("Encoded length: {}, buffer length: {}", encoded_len, buf.len());
        println!("Nibbles took {} bytes", 1 + (nibbles.0.len() + 1) / 2);
        
        // Decode it
        let (decoded, remaining) = StorageTrieEntry::from_compact(&buf, encoded_len);
        
        // Should roundtrip correctly
        assert_eq!(entry.nibbles.0, decoded.nibbles.0);
        assert_eq!(entry.node, decoded.node);
        assert_eq!(remaining.len(), 0);
    }
    
    #[test]
    fn test_storage_trie_entry_with_old_format_nibbles() {
        // Simulate old format: create a buffer with old-style nibbles encoding
        let nibbles_vec = vec![0x01, 0x02, 0x03, 0x04];
        let mut old_format_buf = Vec::new();
        
        // Old format: 64 bytes padded + 1 length byte
        old_format_buf.extend_from_slice(&nibbles_vec);
        old_format_buf.resize(64, 0); // Pad to 64 bytes
        old_format_buf.push(nibbles_vec.len() as u8); // Length at position 64
        
        // Add a default branch node
        let node = BranchNodeCompact::default();
        let mut node_buf = Vec::new();
        let node_len = node.to_compact(&mut node_buf);
        old_format_buf.extend_from_slice(&node_buf);
        
        let total_len = 65 + node_len; // Old format nibbles + node
        
        // Try to decode this old format
        let (decoded, remaining) = StorageTrieEntry::from_compact(&old_format_buf, total_len);
        
        // Should decode correctly
        assert_eq!(decoded.nibbles.0.to_vec(), nibbles_vec);
        assert_eq!(decoded.node, node);
        assert_eq!(remaining.len(), 0);
    }
}
