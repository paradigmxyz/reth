#!/usr/bin/env cargo +nightly -Zscript

//! Test script to verify packed nibble encoding works correctly
//! Run with: cargo +nightly -Zscript test_packed_nibbles.rs
//! 
//! ```cargo
//! [dependencies]
//! reth-trie-common = { path = "crates/trie/common" }
//! reth-codecs = { path = "crates/storage/codecs" }
//! nybbles = "0.2"
//! ```

use reth_trie_common::{StoredNibblesSubKey, storage::StorageTrieEntry, BranchNodeCompact};
use reth_codecs::Compact;
use nybbles::Nibbles;

fn main() {
    println!("Testing packed nibble encoding with StorageTrieEntry...\n");
    
    // Test 1: Small nibbles (typical case)
    test_storage_entry(&[0x01, 0x02, 0x03, 0x04], "Small nibbles (4)");
    
    // Test 2: Empty nibbles
    test_storage_entry(&[], "Empty nibbles");
    
    // Test 3: Single nibble
    test_storage_entry(&[0x0F], "Single nibble");
    
    // Test 4: Maximum nibbles (64)
    let max_nibbles: Vec<u8> = (0..64).map(|i| (i % 16) as u8).collect();
    test_storage_entry(&max_nibbles, "Maximum nibbles (64)");
    
    // Test 5: Odd number of nibbles
    test_storage_entry(&[0x01, 0x02, 0x03, 0x04, 0x05], "Odd nibbles (5)");
    
    // Test 6: Common trie path length (32 nibbles)
    let path_32: Vec<u8> = (0..32).map(|i| (i % 16) as u8).collect();
    test_storage_entry(&path_32, "Common path (32)");
    
    println!("\n✅ All tests passed!");
}

fn test_storage_entry(nibbles_data: &[u8], description: &str) {
    println!("Testing: {}", description);
    
    // Create storage trie entry
    let nibbles = StoredNibblesSubKey::from(nibbles_data.to_vec());
    let node = BranchNodeCompact::new(
        0x1234.into(),  // state_mask
        0x5678.into(),  // tree_mask
        0x9ABC.into(),  // hash_mask
        vec![],         // hashes
        None,           // root_hash
    );
    
    let entry = StorageTrieEntry {
        nibbles: nibbles.clone(),
        node: node.clone(),
    };
    
    // Encode
    let mut buf = Vec::new();
    let encoded_len = entry.to_compact(&mut buf);
    
    // Calculate expected sizes
    let old_size = 65; // Old format always used 65 bytes for nibbles
    let new_nibbles_size = 1 + (nibbles_data.len() + 1) / 2; // New packed format
    let node_size = 6; // 3 masks * 2 bytes each (no hashes in this test)
    
    println!("  Nibbles: {} nibbles", nibbles_data.len());
    println!("  Old format would use: {} bytes for nibbles", old_size);
    println!("  New format uses: {} bytes for nibbles", new_nibbles_size);
    println!("  Total encoded: {} bytes (nibbles: {}, node: {})", 
             encoded_len, new_nibbles_size, node_size);
    
    // Decode
    let (decoded, remaining) = StorageTrieEntry::from_compact(&buf, encoded_len);
    
    // Verify
    assert_eq!(decoded.nibbles.0.len(), nibbles_data.len(), 
               "Nibbles length mismatch for {}", description);
    assert_eq!(decoded.nibbles.0.to_vec(), nibbles_data, 
               "Nibbles data mismatch for {}", description);
    assert_eq!(decoded.node, node, 
               "Node mismatch for {}", description);
    assert_eq!(remaining.len(), 0, 
               "Buffer not fully consumed for {}", description);
    
    // Calculate space savings
    let savings = old_size as i32 - new_nibbles_size as i32;
    let percentage = if old_size > 0 {
        (savings as f64 / old_size as f64) * 100.0
    } else {
        0.0
    };
    
    println!("  Space savings: {} bytes ({:.1}%)", savings, percentage);
    println!("  ✓ Roundtrip successful\n");
}