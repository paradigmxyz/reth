//! Integration test: simulate multiple blocks flowing through the network cache.

use alloy_primitives::{Address, B256, U256};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    network_cache::NetworkStateCache,
    policy::{AccountData, LastNBlocksPolicy},
};

/// Simulate 20 blocks with some overlapping state access patterns.
#[test]
fn test_multi_block_simulation() {
    // Account policy: keep for 10 blocks
    // Storage policy: keep for 5 blocks (more aggressive eviction)
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(10)),
        Box::new(LastNBlocksPolicy::new(5)),
    );

    // Common "hot" addresses (like USDT, WETH) that appear in every block
    let hot_addr = Address::repeat_byte(0xAA);
    let hot_slot = B256::repeat_byte(0x01);

    // Cold addresses that appear only once
    let cold_addrs: Vec<Address> = (0..20).map(|i| Address::repeat_byte(i)).collect();

    let mut total_accessed = 0usize;
    let mut total_missed = 0usize;

    for block in 100..120 {
        let mut accessed = BlockAccessedState::default();

        // Hot address always accessed
        accessed.accounts.insert(
            hot_addr,
            AccountData { nonce: block, balance: U256::from(block * 1000), code_hash: None },
        );
        accessed.storage.insert((hot_addr, hot_slot), U256::from(block));

        // One cold address per block
        let cold = cold_addrs[block as usize - 100];
        accessed.accounts.insert(
            cold,
            AccountData { nonce: 0, balance: U256::from(100), code_hash: None },
        );

        // Compute miss before updating cache
        let miss = cache.compute_miss(&accessed);
        total_accessed += miss.total_accessed;
        total_missed += miss.total_missed;

        // Update cache with this block's data
        let stats = cache.on_block_executed(block, &accessed);

        // Verify hot address is always cached after first block
        if block > 100 {
            // Hot addr should hit cache (was inserted in previous block)
            assert!(
                cache.contains_account(&hot_addr),
                "hot address should be cached at block {block}"
            );
        }

        // Print stats for debugging
        eprintln!(
            "Block {block}: miss_ratio={:.1}%, accounts_cached={}, storage_cached={}, evicted_storage={}",
            miss.miss_ratio * 100.0,
            cache.snapshot().total_accounts,
            cache.snapshot().total_storage_slots,
            stats.storage_evicted,
        );
    }

    // After all blocks:
    let snap = cache.snapshot();

    // Account window=10: hot addr + cold addrs within window
    // Window includes current block and 10 blocks back, so up to 12 accounts at steady state
    // (hot + 11 cold addrs that are within the 10-block window due to inclusive boundary)
    assert!(snap.total_accounts <= 12, "should have at most 12 accounts, got {}", snap.total_accounts);

    // Storage window=5: hot_slot was accessed every block, so it's always retained
    assert!(cache.contains_storage(&hot_addr, &hot_slot));

    // Overall: first block is 100% miss, subsequent blocks should have lower miss
    let overall_miss_ratio = total_missed as f64 / total_accessed as f64;
    eprintln!("Overall miss ratio: {:.1}%", overall_miss_ratio * 100.0);
    // The hot address is always hit after first block, so miss < 100%
    assert!(overall_miss_ratio < 1.0);
}

/// Test that separate policies allow keeping accounts longer than storage.
#[test]
fn test_differentiated_policies() {
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(20)), // accounts: keep 20 blocks
        Box::new(LastNBlocksPolicy::new(3)),  // storage: keep only 3 blocks
    );

    let addr = Address::repeat_byte(0x42);
    let slot = B256::repeat_byte(0x01);

    // Block 10: access account + storage
    let mut accessed = BlockAccessedState::default();
    accessed.accounts.insert(
        addr,
        AccountData { nonce: 1, balance: U256::from(500), code_hash: None },
    );
    accessed.storage.insert((addr, slot), U256::from(99));
    cache.on_block_executed(10, &accessed);

    // Block 14: storage cutoff = 11 (evicts slot accessed at 10)
    //           account cutoff = 0 (still well within window)
    cache.on_block_executed(14, &BlockAccessedState::default());

    assert!(cache.contains_account(&addr), "account should survive with window=20");
    assert!(!cache.contains_storage(&addr, &slot), "storage should be evicted with window=3");
}

/// Test sidecar serialization and deserialization along with build_sidecar_targets logic.
#[test]
fn test_sidecar_serialization_and_targets() {
    use alloy_primitives::Bytes;
    use reth_trie_common::{MultiProof, StorageMultiProof, BranchNodeMasks, TrieMask};
    use reth_trie_common::proof::ProofNodes;
    use alloy_primitives::map::{B256Map, HashMap};
    use partial_stateless::{
        witness::{build_sidecar_targets, WitnessResult},
        sidecar::{WitnessTargets, PartialStatelessSidecar, SerializableMultiProof},
        network_cache::MissResult,
    };

    // 1. Create a dummy MultiProof
    let account = Address::repeat_byte(0x11);
    let hashed_account = alloy_primitives::keccak256(account);
    let slot = B256::repeat_byte(0x22);
    let hashed_slot = alloy_primitives::keccak256(slot);

    let mut account_subtree: HashMap<reth_trie_common::Nibbles, Bytes> = HashMap::default();
    account_subtree.insert(reth_trie_common::Nibbles::unpack(hashed_account), Bytes::from(vec![1, 2, 3]));
    let account_subtree = ProofNodes::from_iter(account_subtree);

    let mut branch_node_masks: HashMap<reth_trie_common::Nibbles, BranchNodeMasks> = HashMap::default();
    branch_node_masks.insert(
        reth_trie_common::Nibbles::unpack(hashed_account),
        BranchNodeMasks {
            hash_mask: TrieMask::new(0b101),
            tree_mask: TrieMask::new(0b010),
        },
    );

    let mut storage_subtree: HashMap<reth_trie_common::Nibbles, Bytes> = HashMap::default();
    storage_subtree.insert(reth_trie_common::Nibbles::unpack(hashed_slot), Bytes::from(vec![4, 5, 6]));
    let storage_subtree = ProofNodes::from_iter(storage_subtree);

    let mut storage_masks: HashMap<reth_trie_common::Nibbles, BranchNodeMasks> = HashMap::default();
    storage_masks.insert(
        reth_trie_common::Nibbles::unpack(hashed_slot),
        BranchNodeMasks {
            hash_mask: TrieMask::new(0b011),
            tree_mask: TrieMask::new(0b100),
        },
    );

    let mut storages = B256Map::default();
    storages.insert(
        hashed_account,
        StorageMultiProof {
            root: B256::repeat_byte(0xAA),
            subtree: storage_subtree,
            branch_node_masks: storage_masks,
        },
    );

    let original_proof = MultiProof {
        account_subtree,
        branch_node_masks,
        storages,
    };

    // 2. Test SerializableMultiProof conversion
    let serializable = SerializableMultiProof::from_multiproof(&original_proof);
    let restored_proof = serializable.to_multiproof();

    assert_eq!(original_proof, restored_proof, "Converted multiproof should match the original");

    // 3. Test bincode serialization of PartialStatelessSidecar
    let raw_targets = WitnessTargets {
        missed_accounts: vec![account],
        missed_storage: vec![(account, slot)],
        missed_code_hashes: vec![B256::repeat_byte(0xCC)],
    };

    let serialized_multiproof = bincode::serialize(&serializable).expect("serialize multiproof");

    let stats = WitnessResult {
        total_size_bytes: 1000,
        account_proof_bytes: 400,
        storage_proof_bytes: 400,
        bytecode_bytes: 200,
        account_proof_nodes: 5,
        storage_proof_nodes: 5,
        target_accounts: 1,
        target_storage_slots: 1,
        computation_time_ms: Some(15),
    };

    let sidecar = PartialStatelessSidecar {
        parent_hash: B256::repeat_byte(0x01),
        parent_state_root: B256::repeat_byte(0x02),
        block_hash: B256::repeat_byte(0x03),
        block_number: 100,
        cache_block: 99,
        cache_policy_metadata: "LastNBlocks(60, 30)".to_string(),
        raw_targets: raw_targets.clone(),
        serialized_multiproof,
        missed_bytecodes: vec![Bytes::from(vec![9, 8, 7])],
        ancestor_headers: vec![],
        stats,
    };

    let sidecar_bytes = bincode::serialize(&sidecar).expect("serialize sidecar");
    let deserialized_sidecar: PartialStatelessSidecar = bincode::deserialize(&sidecar_bytes).expect("deserialize sidecar");

    assert_eq!(deserialized_sidecar.block_number, sidecar.block_number);
    assert_eq!(deserialized_sidecar.raw_targets.missed_accounts, sidecar.raw_targets.missed_accounts);
    assert_eq!(deserialized_sidecar.missed_bytecodes, sidecar.missed_bytecodes);
    assert_eq!(deserialized_sidecar.stats.total_size_bytes, sidecar.stats.total_size_bytes);

    // 4. Test build_sidecar_targets
    let miss = MissResult {
        total_missed: 3,
        total_accessed: 10,
        miss_ratio: 0.3,
        missed_accounts: vec![account],
        missed_storage: vec![(account, slot)],
        missed_codes: vec![B256::repeat_byte(0xCC)],
    };

    let (got_raw_targets, got_multiproof_targets) = build_sidecar_targets(&miss);
    assert_eq!(got_raw_targets, raw_targets);
    assert_eq!(got_multiproof_targets.len(), 1);
    assert!(got_multiproof_targets.contains_key(&hashed_account));
}

/// Test that the sidecar file can be successfully written to disk and read back.
#[test]
fn test_sidecar_disk_write() {
    use std::fs;
    use partial_stateless::{
        sidecar::{WitnessTargets, PartialStatelessSidecar},
        witness::WitnessResult,
    };

    let sidecar_dir = std::env::temp_dir().join("reth_sidecar_test");
    // Ensure clean state for test
    if sidecar_dir.exists() {
        let _ = fs::remove_dir_all(&sidecar_dir);
    }

    let raw_targets = WitnessTargets {
        missed_accounts: vec![Address::repeat_byte(0x11)],
        missed_storage: vec![(Address::repeat_byte(0x11), B256::repeat_byte(0x22))],
        missed_code_hashes: vec![B256::repeat_byte(0xCC)],
    };

    let stats = WitnessResult {
        total_size_bytes: 500,
        account_proof_bytes: 200,
        storage_proof_bytes: 200,
        bytecode_bytes: 100,
        account_proof_nodes: 2,
        storage_proof_nodes: 2,
        target_accounts: 1,
        target_storage_slots: 1,
        computation_time_ms: Some(5),
    };

    let sidecar = PartialStatelessSidecar {
        parent_hash: B256::repeat_byte(0x01),
        parent_state_root: B256::repeat_byte(0x02),
        block_hash: B256::repeat_byte(0x03),
        block_number: 100,
        cache_block: 99,
        cache_policy_metadata: "LastNBlocks(60, 30)".to_string(),
        raw_targets,
        serialized_multiproof: vec![1, 2, 3, 4],
        missed_bytecodes: vec![alloy_primitives::Bytes::from(vec![9, 8, 7])],
        ancestor_headers: vec![],
        stats,
    };

    // Create dir
    fs::create_dir_all(&sidecar_dir).expect("create sidecar dir");
    let file_path = sidecar_dir.join("block_100_0x0300000000000000000000000000000000000000000000000000000000000000.bin");
    let sidecar_bytes = bincode::serialize(&sidecar).expect("serialize sidecar");
    fs::write(&file_path, sidecar_bytes).expect("write sidecar file");

    assert!(file_path.exists(), "Sidecar file should exist on disk");
    assert!(file_path.metadata().unwrap().len() > 0, "Sidecar file should not be empty");

    // Read back and verify
    let read_bytes = fs::read(&file_path).expect("read sidecar file");
    let deserialized: PartialStatelessSidecar = bincode::deserialize(&read_bytes).expect("deserialize sidecar");
    assert_eq!(deserialized.block_number, 100);

    // Clean up after test
    fs::remove_dir_all(&sidecar_dir).expect("cleanup sidecar dir");
}
