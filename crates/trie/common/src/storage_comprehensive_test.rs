#[cfg(test)]
mod comprehensive_tests {
    use super::super::*;
    use crate::{BranchNodeCompact, StoredNibblesSubKey, storage::StorageTrieEntry};
    use reth_codecs::Compact;
    use alloy_trie::TrieMask;

    #[test]
    fn test_storage_entry_with_various_nibble_lengths() {
        // Test various nibble lengths to ensure packed encoding works
        let test_cases = vec![
            (vec![], "empty nibbles"),
            (vec![0x0F], "single nibble"),
            (vec![0x01, 0x02], "two nibbles"),
            (vec![0x01, 0x02, 0x03], "three nibbles"),
            (vec![0x01, 0x02, 0x03, 0x04], "four nibbles"),
            ((0..8).map(|i| (i % 16) as u8).collect::<Vec<_>>(), "8 nibbles"),
            ((0..16).map(|i| (i % 16) as u8).collect::<Vec<_>>(), "16 nibbles"),
            ((0..32).map(|i| (i % 16) as u8).collect::<Vec<_>>(), "32 nibbles"),
            ((0..63).map(|i| (i % 16) as u8).collect::<Vec<_>>(), "63 nibbles"),
            ((0..64).map(|i| (i % 16) as u8).collect::<Vec<_>>(), "64 nibbles (max)"),
        ];

        for (nibbles_data, description) in test_cases {
            println!("\nTesting: {}", description);
            
            // Create storage trie entry
            let nibbles = StoredNibblesSubKey::from(nibbles_data.clone());
            // Use valid masks - tree_mask and hash_mask must be subsets of state_mask
            // hash_mask.count_ones() must equal hashes.len()
            let node = BranchNodeCompact::new(
                TrieMask::new(0xFFFF),  // state_mask: all bits (has all children)
                TrieMask::new(0x00FF),  // tree_mask: subset of state_mask
                TrieMask::new(0x0000),  // hash_mask: no hashes (empty vec)
                vec![],
                None,
            );
            
            let entry = StorageTrieEntry {
                nibbles: nibbles.clone(),
                node: node.clone(),
            };
            
            // Encode
            let mut buf = Vec::new();
            let encoded_len = entry.to_compact(&mut buf);
            
            // Verify encoded size matches expected
            let expected_nibbles_size = 1 + (nibbles_data.len() + 1) / 2;
            let expected_node_size = 6; // 3 masks * 2 bytes
            let expected_total = expected_nibbles_size + expected_node_size;
            
            assert_eq!(encoded_len, expected_total, 
                      "Encoded length mismatch for {}", description);
            assert_eq!(buf.len(), expected_total,
                      "Buffer length mismatch for {}", description);
            
            // Decode
            let (decoded, remaining) = StorageTrieEntry::from_compact(&buf, encoded_len);
            
            // Verify roundtrip
            assert_eq!(decoded.nibbles.0.len(), nibbles_data.len(),
                      "Nibbles length mismatch for {}", description);
            assert_eq!(decoded.nibbles.0.to_vec(), nibbles_data,
                      "Nibbles data mismatch for {}", description);
            assert_eq!(decoded.node, node,
                      "Node mismatch for {}", description);
            assert_eq!(remaining.len(), 0,
                      "Buffer not fully consumed for {}", description);
            
            // Calculate and verify space savings
            let old_size = 65; // Old format always used 65 bytes for nibbles
            let new_size = expected_nibbles_size;
            let savings = old_size - new_size;
            let percentage = (savings as f64 / old_size as f64) * 100.0;
            
            println!("  Old format: {} bytes, New format: {} bytes", old_size, new_size);
            println!("  Space savings: {} bytes ({:.1}%)", savings, percentage);
            println!("  ✓ Test passed");
        }
    }

    #[test]
    fn test_storage_entry_with_hashes() {
        use alloy_primitives::{B256, hex};
        
        // Test with actual hashes in the branch node
        let nibbles = StoredNibblesSubKey::from(vec![0x01, 0x02, 0x03, 0x04]);
        let node = BranchNodeCompact::new(
            TrieMask::new(0xf607),
            TrieMask::new(0x0005),
            TrieMask::new(0x4004),
            vec![
                hex!("90d53cd810cc5d4243766cd4451e7b9d14b736a1148b26b3baac7617f617d321").into(),
                hex!("cc35c964dda53ba6c0b87798073a9628dbc9cd26b5cce88eb69655a9c609caf1").into(),
            ],
            Some(hex!("aaaabbbb0006767767776fffffeee44444000005567645600000000eeddddddd").into()),
        );
        
        let entry = StorageTrieEntry {
            nibbles: nibbles.clone(),
            node: node.clone(),
        };
        
        // Encode
        let mut buf = Vec::new();
        let encoded_len = entry.to_compact(&mut buf);
        
        // Expected sizes:
        // - Nibbles: 1 byte length + 2 bytes packed (4 nibbles / 2)
        // - Node: 6 bytes masks + 32*3 bytes hashes = 102 bytes
        let expected_nibbles_size = 3;
        let expected_node_size = 6 + 32 * 3;
        let expected_total = expected_nibbles_size + expected_node_size;
        
        assert_eq!(encoded_len, expected_total, "Encoded length mismatch with hashes");
        
        // Decode and verify
        let (decoded, remaining) = StorageTrieEntry::from_compact(&buf, encoded_len);
        
        assert_eq!(decoded.nibbles.0.to_vec(), vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(decoded.node, node);
        assert_eq!(remaining.len(), 0);
        
        println!("✓ Storage entry with hashes roundtrips correctly");
    }

    #[test]
    fn test_backwards_compatibility_with_real_old_data() {
        // Simulate real old format data from database
        let mut old_format_buf = Vec::new();
        
        // Old format nibbles: 4 nibbles padded to 64 bytes + length
        let nibbles_vec = vec![0x0A, 0x0B, 0x0C, 0x0D];
        old_format_buf.extend_from_slice(&nibbles_vec);
        old_format_buf.resize(64, 0); // Pad to 64 bytes
        old_format_buf.push(4); // Length at position 64
        
        // Add a branch node with valid masks
        let node = BranchNodeCompact::new(
            TrieMask::new(0xFFFF),  // state_mask: all children present
            TrieMask::new(0x00FF),  // tree_mask: subset of state_mask
            TrieMask::new(0x0000),  // hash_mask: no hashes (empty vec)
            vec![],
            None,
        );
        let mut node_buf = Vec::new();
        let node_len = node.to_compact(&mut node_buf);
        old_format_buf.extend_from_slice(&node_buf);
        
        let total_len = 65 + node_len;
        
        // This simulates reading old format from database
        let (decoded, remaining) = StorageTrieEntry::from_compact(&old_format_buf, total_len);
        
        // Verify it decoded correctly
        assert_eq!(decoded.nibbles.0.to_vec(), nibbles_vec);
        assert_eq!(decoded.node, node);
        assert_eq!(remaining.len(), 0);
        
        // Now encode it with new format
        let mut new_buf = Vec::new();
        let new_len = decoded.to_compact(&mut new_buf);
        
        // Should be much smaller: 1 + 2 (nibbles) + 6 (node) = 9 bytes
        assert_eq!(new_len, 9);
        assert!(new_len < total_len, "New format should be smaller");
        
        // And it should roundtrip with new format
        let (decoded2, remaining2) = StorageTrieEntry::from_compact(&new_buf, new_len);
        assert_eq!(decoded2.nibbles.0.to_vec(), nibbles_vec);
        assert_eq!(decoded2.node, node);
        assert_eq!(remaining2.len(), 0);
        
        println!("✓ Backwards compatibility verified: old format reads correctly and converts to new format");
    }

    #[test]
    fn test_storage_entry_variable_nibble_lengths_with_hashes() {
        // Test case that would cause buf.len() % 32 != 6 after nibbles
        // This simulates the exact error scenario from production
        
        // Case 1: 1 nibble = 2 bytes total (1 length + 1 packed)
        // After nibbles, remaining buffer for BranchNodeCompact would not align to % 32 == 6
        test_with_nibble_count_and_hashes(1);
        
        // Case 2: 3 nibbles = 3 bytes total (1 length + 2 packed)
        test_with_nibble_count_and_hashes(3);
        
        // Case 3: 5 nibbles = 4 bytes total (1 length + 3 packed)
        test_with_nibble_count_and_hashes(5);
        
        // Case 4: 17 nibbles = 10 bytes total (1 length + 9 packed)
        test_with_nibble_count_and_hashes(17);
        
        println!("✓ All variable nibble length tests with hashes passed");
        
        fn test_with_nibble_count_and_hashes(nibble_count: usize) {
            use alloy_primitives::B256;
            use crate::{BranchNodeCompact, StoredNibblesSubKey, storage::StorageTrieEntry};
            use reth_codecs::Compact;
            use alloy_trie::TrieMask;
            
            let nibbles_data: Vec<u8> = (0..nibble_count).map(|i| (i % 16) as u8).collect();
            let nibbles = StoredNibblesSubKey::from(nibbles_data.clone());
            
            // Create a branch node with actual hashes
            let hash1 = B256::from([0x11; 32]);
            let hash2 = B256::from([0x22; 32]);
            
            let node = BranchNodeCompact::new(
                TrieMask::new(0xFFFF),  // All children present
                TrieMask::new(0x00FF),  // Some are tree nodes
                TrieMask::new(0x0003),  // Two hashes (bits 0 and 1)
                vec![hash1, hash2],
                None,
            );
            
            let entry = StorageTrieEntry {
                nibbles: nibbles.clone(),
                node: node.clone(),
            };
            
            // Encode
            let mut buf = Vec::new();
            let encoded_len = entry.to_compact(&mut buf);
            
            // The nibbles will take: 1 + (nibble_count + 1) / 2 bytes
            let nibbles_size = 1 + (nibble_count + 1) / 2;
            // The node will take: 6 bytes (masks) + 64 bytes (2 hashes)
            let node_size = 6 + 64;
            let expected_total = nibbles_size + node_size;
            
            assert_eq!(encoded_len, expected_total, 
                      "Encoded length mismatch for {} nibbles", nibble_count);
            
            // This is the critical test - decoding should work even though 
            // nibbles_size might not align nicely with the 32-byte hash boundary
            let (decoded, remaining) = StorageTrieEntry::from_compact(&buf, encoded_len);
            
            assert_eq!(decoded.nibbles.0.to_vec(), nibbles_data,
                      "Nibbles mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.state_mask, node.state_mask,
                      "State mask mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.tree_mask, node.tree_mask,
                      "Tree mask mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.hash_mask, node.hash_mask,
                      "Hash mask mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.hashes.len(), 2,
                      "Hash count mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.hashes[0], hash1,
                      "First hash mismatch for {} nibbles", nibble_count);
            assert_eq!(decoded.node.hashes[1], hash2,
                      "Second hash mismatch for {} nibbles", nibble_count);
            assert_eq!(remaining.len(), 0,
                      "Buffer not fully consumed for {} nibbles", nibble_count);
            
            println!("  ✓ {} nibbles ({} bytes) + hashes roundtrip successful", 
                     nibble_count, nibbles_size);
        }
    }
}