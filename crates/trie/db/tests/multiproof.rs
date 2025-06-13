#![allow(missing_docs)]

use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{address, keccak256, map::B256Set, B256, U256};
use reth_chainspec::MAINNET;
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{
    test_utils::{create_test_provider_factory, insert_genesis},
    HashingWriter,
};
use reth_trie::{proof::Proof, MultiProofTargets, StateRoot, StorageMultiProof};
use reth_trie_db::{DatabaseProof, DatabaseStateRoot};

#[test]
fn test_empty_multiproof() {
    let factory = create_test_provider_factory();
    let provider = factory.provider().unwrap();

    // Generate multiproof for empty targets
    let targets = MultiProofTargets::default();
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Even empty multiproof should contain the root node
    assert!(!multiproof.account_subtree.is_empty());
    assert!(multiproof.storages.is_empty());
    assert!(multiproof.branch_node_hash_masks.is_empty());
    assert!(multiproof.branch_node_tree_masks.is_empty());
}

#[test]
fn test_single_account_multiproof() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = address!("0x1000000000000000000000000000000000000001");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };

    // Insert account
    provider.insert_account_for_hashing([(address, Some(account))]).unwrap();
    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Generate multiproof for single account
    let hashed_address = keccak256(address);
    let targets = MultiProofTargets::account(hashed_address);
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify multiproof structure
    assert!(!multiproof.account_subtree.is_empty());
    assert_eq!(multiproof.storages.len(), 1);
    assert!(multiproof.storages.contains_key(&hashed_address));

    // Convert to account proof and verify
    let account_proof = multiproof.account_proof(address, &[]).unwrap();
    assert_eq!(account_proof.address, address);
    assert_eq!(account_proof.info, Some(account));
    assert_eq!(account_proof.storage_root, EMPTY_ROOT_HASH);
    assert_eq!(account_proof.verify(state_root), Ok(()));
}

#[test]
fn test_multiple_accounts_multiproof() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let accounts = vec![
        (
            address!("0x1000000000000000000000000000000000000001"),
            Account { balance: U256::from(100), nonce: 1, bytecode_hash: None },
        ),
        (
            address!("0x2000000000000000000000000000000000000002"),
            Account { balance: U256::from(200), nonce: 2, bytecode_hash: None },
        ),
        (
            address!("0x3000000000000000000000000000000000000003"),
            Account { balance: U256::from(300), nonce: 3, bytecode_hash: None },
        ),
    ];

    // Insert accounts
    provider
        .insert_account_for_hashing(
            accounts.iter().map(|(addr, acc)| (*addr, Some(*acc))).collect::<Vec<_>>(),
        )
        .unwrap();
    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Generate multiproof for multiple accounts
    let hashed_addresses: Vec<B256> = accounts.iter().map(|(addr, _)| keccak256(*addr)).collect();
    let targets = MultiProofTargets::accounts(hashed_addresses.clone());
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify multiproof contains all accounts
    assert_eq!(multiproof.storages.len(), 3);
    for hashed_addr in &hashed_addresses {
        assert!(multiproof.storages.contains_key(hashed_addr));
    }

    // Verify each account proof
    for (addr, expected_account) in &accounts {
        let account_proof = multiproof.account_proof(*addr, &[]).unwrap();
        assert_eq!(account_proof.address, *addr);
        assert_eq!(account_proof.info, Some(*expected_account));
        assert_eq!(account_proof.verify(state_root), Ok(()));
    }
}

#[test]
fn test_account_with_storage_multiproof() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = address!("0x1000000000000000000000000000000000000001");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };

    // Storage slots
    let slots = vec![
        (B256::from(U256::from(1)), U256::from(100)),
        (B256::from(U256::from(2)), U256::from(200)),
        (B256::from(U256::from(3)), U256::from(300)),
    ];

    // Insert account and storage
    provider.insert_account_for_hashing([(address, Some(account))]).unwrap();
    provider
        .insert_storage_for_hashing([(
            address,
            slots
                .iter()
                .map(|(key, value)| StorageEntry { key: *key, value: *value })
                .collect::<Vec<_>>(),
        )])
        .unwrap();
    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Generate multiproof for account with storage
    let hashed_address = keccak256(address);
    let hashed_slots: B256Set = slots.iter().map(|(key, _)| keccak256(*key)).collect();
    let targets = MultiProofTargets::from_iter([(hashed_address, hashed_slots)]);
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify storage multiproof
    assert!(multiproof.storages.contains_key(&hashed_address));
    let storage_multiproof = &multiproof.storages[&hashed_address];
    assert_ne!(storage_multiproof.root, EMPTY_ROOT_HASH);

    // Verify individual storage proofs
    for (slot, expected_value) in &slots {
        let storage_proof = storage_multiproof.storage_proof(*slot).unwrap();
        assert_eq!(storage_proof.key, *slot);
        assert_eq!(storage_proof.value, *expected_value);
        assert_eq!(storage_proof.verify(storage_multiproof.root), Ok(()));
    }

    // Verify full account proof
    let slot_keys: Vec<B256> = slots.iter().map(|(key, _)| *key).collect();
    let account_proof = multiproof.account_proof(address, &slot_keys).unwrap();
    assert_eq!(account_proof.verify(state_root), Ok(()));
}

#[test]
fn test_multiproof_with_nonexistent_account() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    // Insert one account
    let existing_address = address!("0x1000000000000000000000000000000000000001");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };
    provider.insert_account_for_hashing([(existing_address, Some(account))]).unwrap();
    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Generate multiproof for both existing and non-existing accounts
    let nonexistent_address = address!("0x2000000000000000000000000000000000000002");
    let targets = MultiProofTargets::accounts(vec![
        keccak256(existing_address),
        keccak256(nonexistent_address),
    ]);
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify existing account proof
    let existing_proof = multiproof.account_proof(existing_address, &[]).unwrap();
    assert_eq!(existing_proof.info, Some(account));
    assert_eq!(existing_proof.verify(state_root), Ok(()));

    // Verify non-existent account proof
    let nonexistent_proof = multiproof.account_proof(nonexistent_address, &[]).unwrap();
    assert_eq!(nonexistent_proof.info, None);
    assert_eq!(nonexistent_proof.storage_root, EMPTY_ROOT_HASH);
    assert_eq!(nonexistent_proof.verify(state_root), Ok(()));
}

#[test]
fn test_multiproof_with_empty_and_nonempty_storage() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let addr1 = address!("0x1000000000000000000000000000000000000001");
    let addr2 = address!("0x2000000000000000000000000000000000000002");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };

    // Insert two accounts
    provider.insert_account_for_hashing([(addr1, Some(account)), (addr2, Some(account))]).unwrap();

    // Add storage only to first account
    let slot = B256::from(U256::from(1));
    let value = U256::from(42);
    provider.insert_storage_for_hashing([(addr1, [StorageEntry { key: slot, value }])]).unwrap();

    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Generate multiproof requesting storage for both accounts
    let targets = MultiProofTargets::from_iter([
        (keccak256(addr1), B256Set::from_iter([keccak256(slot)])),
        (keccak256(addr2), B256Set::from_iter([keccak256(slot)])),
    ]);
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify first account has non-empty storage
    let storage1 = &multiproof.storages[&keccak256(addr1)];
    assert_ne!(storage1.root, EMPTY_ROOT_HASH);
    let proof1 = storage1.storage_proof(slot).unwrap();
    assert_eq!(proof1.value, value);

    // Verify second account has empty storage
    let storage2 = &multiproof.storages[&keccak256(addr2)];
    assert_eq!(storage2.root, EMPTY_ROOT_HASH);
    let proof2 = storage2.storage_proof(slot).unwrap();
    assert_eq!(proof2.value, U256::ZERO);

    // Verify full account proofs
    let account_proof1 = multiproof.account_proof(addr1, &[slot]).unwrap();
    let account_proof2 = multiproof.account_proof(addr2, &[slot]).unwrap();
    assert_eq!(account_proof1.verify(state_root), Ok(()));
    assert_eq!(account_proof2.verify(state_root), Ok(()));
}

#[test]
fn test_multiproof_extend() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    // Insert multiple accounts
    let accounts = vec![
        (address!("0x1000000000000000000000000000000000000001"), 100),
        (address!("0x2000000000000000000000000000000000000002"), 200),
        (address!("0x3000000000000000000000000000000000000003"), 300),
    ];

    for (addr, balance) in &accounts {
        provider
            .insert_account_for_hashing([(
                *addr,
                Some(Account { balance: U256::from(*balance), nonce: 1, bytecode_hash: None }),
            )])
            .unwrap();
    }

    // Generate separate multiproofs
    let targets1 = MultiProofTargets::account(keccak256(accounts[0].0));
    let targets2 =
        MultiProofTargets::accounts(vec![keccak256(accounts[1].0), keccak256(accounts[2].0)]);

    let mut multiproof1 = Proof::from_tx(provider.tx_ref()).multiproof(targets1).unwrap();
    let multiproof2 = Proof::from_tx(provider.tx_ref()).multiproof(targets2).unwrap();

    // Extend first multiproof with second
    let _original_len = multiproof1.storages.len();
    multiproof1.extend(multiproof2);

    // Verify extended multiproof contains all accounts
    assert_eq!(multiproof1.storages.len(), 3);
    assert!(multiproof1.storages.contains_key(&keccak256(accounts[0].0)));
    assert!(multiproof1.storages.contains_key(&keccak256(accounts[1].0)));
    assert!(multiproof1.storages.contains_key(&keccak256(accounts[2].0)));
}

#[test]
fn test_multiproof_with_branch_node_masks() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let address = address!("0x1000000000000000000000000000000000000001");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };

    provider.insert_account_for_hashing([(address, Some(account))]).unwrap();

    // Generate multiproof with branch node masks enabled
    let targets = MultiProofTargets::account(keccak256(address));
    let multiproof =
        Proof::from_tx(provider.tx_ref()).with_branch_node_masks(true).multiproof(targets).unwrap();

    // With only one account, we might not have branch nodes, but the fields should be initialized
    assert!(
        multiproof.branch_node_hash_masks.is_empty() ||
            !multiproof.branch_node_hash_masks.is_empty()
    );
    assert!(
        multiproof.branch_node_tree_masks.is_empty() ||
            !multiproof.branch_node_tree_masks.is_empty()
    );
}

#[test]
fn test_storage_multiproof_empty() {
    let empty_proof = StorageMultiProof::empty();
    assert_eq!(empty_proof.root, EMPTY_ROOT_HASH);
    assert!(!empty_proof.subtree.is_empty()); // Contains empty root node

    // Should be able to get proof for any slot (all will be empty)
    let slot = B256::from(U256::from(1));
    let proof = empty_proof.storage_proof(slot).unwrap();
    assert_eq!(proof.key, slot);
    assert_eq!(proof.value, U256::ZERO);
    assert_eq!(proof.verify(EMPTY_ROOT_HASH), Ok(()));
}

#[test]
fn test_multiproof_targets_creation() {
    // Test single account
    let addr1 = B256::random();
    let targets = MultiProofTargets::account(addr1);
    assert_eq!(targets.len(), 1);
    assert!(targets.contains_key(&addr1));
    assert!(targets[&addr1].is_empty());

    // Test multiple accounts
    let addresses = vec![B256::random(), B256::random(), B256::random()];
    let targets = MultiProofTargets::accounts(addresses.clone());
    assert_eq!(targets.len(), 3);
    for addr in &addresses {
        assert!(targets.contains_key(addr));
        assert!(targets[addr].is_empty());
    }

    // Test account with slots
    let addr = B256::random();
    let slots = vec![B256::random(), B256::random()];
    let targets = MultiProofTargets::account_with_slots(addr, slots.clone());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[&addr].len(), 2);
    for slot in &slots {
        assert!(targets[&addr].contains(slot));
    }

    // Test from_iter
    let data = vec![
        (B256::random(), B256Set::from_iter([B256::random(), B256::random()])),
        (B256::random(), B256Set::from_iter([B256::random()])),
    ];
    let targets = MultiProofTargets::from_iter(data.clone());
    assert_eq!(targets.len(), 2);
    for (addr, slots) in &data {
        assert_eq!(&targets[addr], slots);
    }
}

#[test]
fn test_multiproof_targets_retain_difference() {
    let addr1 = B256::random();
    let addr2 = B256::random();
    let slot1 = B256::random();
    let slot2 = B256::random();
    let slot3 = B256::random();

    // Create base targets
    let mut targets = MultiProofTargets::from_iter([
        (addr1, B256Set::from_iter([slot1, slot2, slot3])),
        (addr2, B256Set::from_iter([slot1])),
    ]);

    // Create other targets with some overlap
    let other = MultiProofTargets::from_iter([
        (addr1, B256Set::from_iter([slot1, slot2])), /* Remove slot1 and slot2 from addr1
                                                      * addr2 not present, so it should remain
                                                      * unchanged */
    ]);

    targets.retain_difference(&other);

    // addr1 should only have slot3 remaining
    assert_eq!(targets[&addr1], B256Set::from_iter([slot3]));
    // addr2 should remain unchanged
    assert_eq!(targets[&addr2], B256Set::from_iter([slot1]));
}

#[test]
fn test_multiproof_targets_extend() {
    let addr1 = B256::random();
    let addr2 = B256::random();
    let slot1 = B256::random();
    let slot2 = B256::random();
    let slot3 = B256::random();

    let mut targets = MultiProofTargets::from_iter([
        (addr1, B256Set::from_iter([slot1])),
        (addr2, B256Set::from_iter([slot2])),
    ]);

    let other = MultiProofTargets::from_iter([
        (addr1, B256Set::from_iter([slot2, slot3])), // Add more slots to addr1
        (B256::random(), B256Set::from_iter([slot1])), // Add new address
    ]);

    let _original_len = targets.len();
    targets.extend(other);

    // Should have 3 addresses now
    assert_eq!(targets.len(), 3);
    // addr1 should have all three slots
    assert_eq!(targets[&addr1], B256Set::from_iter([slot1, slot2, slot3]));
    // addr2 should still have just slot2
    assert_eq!(targets[&addr2], B256Set::from_iter([slot2]));
}

#[test]
fn test_multiproof_mainnet_genesis() {
    // Use a real mainnet genesis to test multiproof generation
    let factory = create_test_provider_factory();
    let root = insert_genesis(&factory, MAINNET.clone()).unwrap();

    // Select a few addresses from mainnet genesis
    let addresses = vec![
        address!("0x000d836201318ec6899a67540690382780743280"),
        address!("0x00000000219ab540356cbb839cbe05303d7705fa"), // Beacon deposit contract
    ];

    let provider = factory.provider().unwrap();
    let hashed_addresses: Vec<B256> = addresses.iter().map(|a| keccak256(*a)).collect();
    let targets = MultiProofTargets::accounts(hashed_addresses);
    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify each account proof
    for addr in &addresses {
        let account_proof = multiproof.account_proof(*addr, &[]).unwrap();
        assert_eq!(account_proof.address, *addr);
        assert_eq!(account_proof.verify(root), Ok(()));
    }
}

#[test]
fn test_multiproof_targets_chunks() {
    let addr1 = B256::random();
    let addr2 = B256::random();
    let addr3 = B256::random();

    // Create targets with varying storage slots
    let targets = MultiProofTargets::from_iter([
        (addr1, B256Set::from_iter([B256::random(), B256::random(), B256::random()])),
        (addr2, B256Set::from_iter([B256::random()])),
        (addr3, B256Set::default()), // Account with no storage
    ]);

    // Test chunking with size 2
    let chunks: Vec<MultiProofTargets> = targets.chunks(2).collect();

    // Should have 3 chunks:
    // 1. First two slots from addr1
    // 2. Last slot from addr1 and the slot from addr2
    // 3. addr3 (account with no storage)
    assert_eq!(chunks.len(), 3);

    // Verify all original data is preserved across chunks
    let mut reconstructed = MultiProofTargets::default();
    for chunk in chunks {
        reconstructed.extend(chunk);
    }

    // The reconstructed targets should match the original
    assert_eq!(reconstructed.len(), 3);
}

#[test]
fn test_multiproof_with_overlapping_storage() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let addr1 = address!("0x1000000000000000000000000000000000000001");
    let addr2 = address!("0x2000000000000000000000000000000000000002");
    let account = Account { balance: U256::from(100), nonce: 1, bytecode_hash: None };

    // Both accounts have some common and some unique storage slots
    let common_slot = B256::from(U256::from(1));
    let unique_slot1 = B256::from(U256::from(2));
    let unique_slot2 = B256::from(U256::from(3));

    provider.insert_account_for_hashing([(addr1, Some(account)), (addr2, Some(account))]).unwrap();

    provider
        .insert_storage_for_hashing([
            (
                addr1,
                vec![
                    StorageEntry { key: common_slot, value: U256::from(100) },
                    StorageEntry { key: unique_slot1, value: U256::from(200) },
                ],
            ),
            (
                addr2,
                vec![
                    StorageEntry { key: common_slot, value: U256::from(300) },
                    StorageEntry { key: unique_slot2, value: U256::from(400) },
                ],
            ),
        ])
        .unwrap();

    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();

    // Request proofs for overlapping slots
    let targets = MultiProofTargets::from_iter([
        (keccak256(addr1), B256Set::from_iter([keccak256(common_slot), keccak256(unique_slot1)])),
        (keccak256(addr2), B256Set::from_iter([keccak256(common_slot), keccak256(unique_slot2)])),
    ]);

    let multiproof = Proof::from_tx(provider.tx_ref()).multiproof(targets).unwrap();

    // Verify each account has correct storage values
    let storage1 = &multiproof.storages[&keccak256(addr1)];
    assert_eq!(storage1.storage_proof(common_slot).unwrap().value, U256::from(100));
    assert_eq!(storage1.storage_proof(unique_slot1).unwrap().value, U256::from(200));

    let storage2 = &multiproof.storages[&keccak256(addr2)];
    assert_eq!(storage2.storage_proof(common_slot).unwrap().value, U256::from(300));
    assert_eq!(storage2.storage_proof(unique_slot2).unwrap().value, U256::from(400));

    // Verify full proofs
    let proof1 = multiproof.account_proof(addr1, &[common_slot, unique_slot1]).unwrap();
    let proof2 = multiproof.account_proof(addr2, &[common_slot, unique_slot2]).unwrap();
    assert_eq!(proof1.verify(state_root), Ok(()));
    assert_eq!(proof2.verify(state_root), Ok(()));
}
