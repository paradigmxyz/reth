use super::{BranchNodeCompact, Nibbles, StoredNibblesSubKey};

/// Account storage trie node.
///
/// `nibbles` is the subkey when used as a value in the `StorageTrie` table.
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
        let (nibbles, buf) = StoredNibblesSubKey::from_compact(buf, 65);
        let (node, buf) = BranchNodeCompact::from_compact(buf, len - 65);
        let this = Self { nibbles, node };
        (this, buf)
    }
}

/// Trie changeset entry representing the state of a trie node before a block.
///
/// `nibbles` is the subkey when used as a value in the changeset tables.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct TrieChangeSetsEntry {
    /// The nibbles of the intermediate node
    pub nibbles: StoredNibblesSubKey,
    /// Node value prior to the block being processed, None indicating it didn't exist.
    pub node: Option<BranchNodeCompact>,
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for TrieChangeSetsEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.as_ref().map(|node| node.to_compact(buf)).unwrap_or(0);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            // Return an empty entry without trying to parse anything
            return (
                Self { nibbles: StoredNibblesSubKey::from(Nibbles::default()), node: None },
                buf,
            )
        }

        let (nibbles, buf) = StoredNibblesSubKey::from_compact(buf, 65);

        if len <= 65 {
            return (Self { nibbles, node: None }, buf)
        }

        let (node, buf) = BranchNodeCompact::from_compact(buf, len - 65);
        (Self { nibbles, node: Some(node) }, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use reth_codecs::Compact;

    #[test]
    fn test_trie_changesets_entry_full_empty() {
        // Test a fully empty entry (empty nibbles, None node)
        let entry = TrieChangeSetsEntry { nibbles: StoredNibblesSubKey::from(vec![]), node: None };

        let mut buf = BytesMut::new();
        let len = entry.to_compact(&mut buf);

        // Empty nibbles takes 65 bytes (64 for padding + 1 for length)
        // None node adds 0 bytes
        assert_eq!(len, 65);
        assert_eq!(buf.len(), 65);

        // Deserialize and verify
        let (decoded, remaining) = TrieChangeSetsEntry::from_compact(&buf, len);
        assert_eq!(decoded.nibbles.0.to_vec(), Vec::<u8>::new());
        assert_eq!(decoded.node, None);
        assert_eq!(remaining.len(), 0);
    }

    #[test]
    fn test_trie_changesets_entry_none_node() {
        // Test non-empty nibbles with None node
        let nibbles_data = vec![0x01, 0x02, 0x03, 0x04];
        let entry = TrieChangeSetsEntry {
            nibbles: StoredNibblesSubKey::from(nibbles_data.clone()),
            node: None,
        };

        let mut buf = BytesMut::new();
        let len = entry.to_compact(&mut buf);

        // Nibbles takes 65 bytes regardless of content
        assert_eq!(len, 65);

        // Deserialize and verify
        let (decoded, remaining) = TrieChangeSetsEntry::from_compact(&buf, len);
        assert_eq!(decoded.nibbles.0.to_vec(), nibbles_data);
        assert_eq!(decoded.node, None);
        assert_eq!(remaining.len(), 0);
    }

    #[test]
    fn test_trie_changesets_entry_empty_path_with_node() {
        // Test empty path with Some node
        // Using the same signature as in the codebase: (state_mask, hash_mask, tree_mask, hashes,
        // value)
        let test_node = BranchNodeCompact::new(
            0b1111_1111_1111_1111, // state_mask: all children present
            0b1111_1111_1111_1111, // hash_mask: all have hashes
            0b0000_0000_0000_0000, // tree_mask: no embedded trees
            vec![],                // hashes
            None,                  // value
        );

        let entry = TrieChangeSetsEntry {
            nibbles: StoredNibblesSubKey::from(vec![]),
            node: Some(test_node.clone()),
        };

        let mut buf = BytesMut::new();
        let len = entry.to_compact(&mut buf);

        // Calculate expected length
        let mut temp_buf = BytesMut::new();
        let node_len = test_node.to_compact(&mut temp_buf);
        assert_eq!(len, 65 + node_len);

        // Deserialize and verify
        let (decoded, remaining) = TrieChangeSetsEntry::from_compact(&buf, len);
        assert_eq!(decoded.nibbles.0.to_vec(), Vec::<u8>::new());
        assert_eq!(decoded.node, Some(test_node));
        assert_eq!(remaining.len(), 0);
    }

    #[test]
    fn test_trie_changesets_entry_normal() {
        // Test normal case: non-empty path with Some node
        let nibbles_data = vec![0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f];
        // Using the same signature as in the codebase
        let test_node = BranchNodeCompact::new(
            0b0000_0000_1111_0000, // state_mask: some children present
            0b0000_0000_0011_0000, // hash_mask: some have hashes
            0b0000_0000_0000_0000, // tree_mask: no embedded trees
            vec![],                // hashes (empty for this test)
            None,                  // value
        );

        let entry = TrieChangeSetsEntry {
            nibbles: StoredNibblesSubKey::from(nibbles_data.clone()),
            node: Some(test_node.clone()),
        };

        let mut buf = BytesMut::new();
        let len = entry.to_compact(&mut buf);

        // Verify serialization length
        let mut temp_buf = BytesMut::new();
        let node_len = test_node.to_compact(&mut temp_buf);
        assert_eq!(len, 65 + node_len);

        // Deserialize and verify
        let (decoded, remaining) = TrieChangeSetsEntry::from_compact(&buf, len);
        assert_eq!(decoded.nibbles.0.to_vec(), nibbles_data);
        assert_eq!(decoded.node, Some(test_node));
        assert_eq!(remaining.len(), 0);
    }

    #[test]
    fn test_trie_changesets_entry_from_compact_zero_len() {
        // Test from_compact with zero length
        let buf = vec![0x01, 0x02, 0x03];
        let (decoded, remaining) = TrieChangeSetsEntry::from_compact(&buf, 0);

        // Should return empty nibbles and None node
        assert_eq!(decoded.nibbles.0.to_vec(), Vec::<u8>::new());
        assert_eq!(decoded.node, None);
        assert_eq!(remaining, &buf[..]); // Buffer should be unchanged
    }
}
