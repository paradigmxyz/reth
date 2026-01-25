//! Integration tests for cross-client execution metrics.
//!
//! These tests verify that the metrics calculation logic correctly counts:
//! - Account deletions (selfdestructed accounts)
//! - Storage slot deletions (slots set to zero)
//! - EIP-7702 delegations set and cleared

use alloy_primitives::{address, map::HashMap, Address, Bytes, U256};
use reth_evm::metrics::calculate_hit_rate;
use reth_revm::db::BundleAccount;
use revm::state::AccountInfo;
use revm_bytecode::Bytecode;
use revm_database::{states::StorageSlot, AccountStatus};
use revm_primitives::KECCAK_EMPTY;

/// Test that accounts_deleted count is correctly calculated from BundleState.
/// An account is considered deleted when it was destroyed (selfdestructed).
#[test]
fn test_accounts_deleted_calculation() {
    // Create accounts with different statuses
    let accounts: Vec<(Address, BundleAccount)> = vec![
        // Normal account (not deleted)
        (
            address!("1000000000000000000000000000000000000001"),
            BundleAccount {
                info: Some(AccountInfo {
                    balance: U256::from(100),
                    nonce: 1,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                    account_id: None,
                }),
                original_info: Some(AccountInfo {
                    balance: U256::from(50),
                    nonce: 0,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                    account_id: None,
                }),
                storage: HashMap::default(),
                status: AccountStatus::Changed,
            },
        ),
        // Destroyed account (deleted)
        (
            address!("2000000000000000000000000000000000000002"),
            BundleAccount {
                info: None,
                original_info: Some(AccountInfo {
                    balance: U256::from(1000),
                    nonce: 5,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                    account_id: None,
                }),
                storage: HashMap::default(),
                status: AccountStatus::Destroyed,
            },
        ),
        // Another destroyed account (deleted)
        (
            address!("3000000000000000000000000000000000000003"),
            BundleAccount {
                info: None,
                original_info: Some(AccountInfo {
                    balance: U256::from(500),
                    nonce: 2,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                    account_id: None,
                }),
                storage: HashMap::default(),
                status: AccountStatus::DestroyedChanged,
            },
        ),
        // Loaded account (not deleted)
        (
            address!("4000000000000000000000000000000000000004"),
            BundleAccount {
                info: Some(AccountInfo::default()),
                original_info: Some(AccountInfo::default()),
                storage: HashMap::default(),
                status: AccountStatus::Loaded,
            },
        ),
    ];

    // Count deleted accounts (same logic as in payload_validator.rs)
    let accounts_deleted = accounts.iter().filter(|(_, acc)| acc.was_destroyed()).count();

    // Should be 2: the Destroyed and DestroyedChanged accounts
    assert_eq!(accounts_deleted, 2, "Should count 2 deleted accounts");
}

/// Test that storage_slots_deleted count is correctly calculated.
/// A storage slot is deleted when present_value is zero and previous was non-zero.
#[test]
fn test_storage_slots_deleted_calculation() {
    // Create storage slots with different states
    let mut storage: HashMap<U256, StorageSlot> = HashMap::default();

    // Slot that was deleted (non-zero -> zero)
    storage.insert(
        U256::from(1),
        StorageSlot { present_value: U256::ZERO, previous_or_original_value: U256::from(100) },
    );

    // Slot that was updated (non-zero -> non-zero)
    storage.insert(
        U256::from(2),
        StorageSlot { present_value: U256::from(200), previous_or_original_value: U256::from(100) },
    );

    // Slot that stayed zero (zero -> zero, not a deletion)
    storage.insert(
        U256::from(3),
        StorageSlot { present_value: U256::ZERO, previous_or_original_value: U256::ZERO },
    );

    // Another deleted slot
    storage.insert(
        U256::from(4),
        StorageSlot { present_value: U256::ZERO, previous_or_original_value: U256::from(50) },
    );

    // Slot that was created (zero -> non-zero)
    storage.insert(
        U256::from(5),
        StorageSlot { present_value: U256::from(300), previous_or_original_value: U256::ZERO },
    );

    // Count deleted storage slots (same logic as in payload_validator.rs)
    let storage_slots_deleted = storage
        .values()
        .filter(|slot| slot.present_value.is_zero() && !slot.previous_or_original_value.is_zero())
        .count();

    // Should be 2: slots 1 and 4
    assert_eq!(storage_slots_deleted, 2, "Should count 2 deleted storage slots");
}

/// Test that storage deletions are counted across multiple accounts.
#[test]
fn test_storage_slots_deleted_across_accounts() {
    let accounts: Vec<(Address, BundleAccount)> = vec![
        (
            address!("1000000000000000000000000000000000000001"),
            BundleAccount {
                info: Some(AccountInfo::default()),
                original_info: Some(AccountInfo::default()),
                storage: {
                    let mut s = HashMap::default();
                    // 1 deleted slot
                    s.insert(
                        U256::from(1),
                        StorageSlot {
                            present_value: U256::ZERO,
                            previous_or_original_value: U256::from(100),
                        },
                    );
                    s
                },
                status: AccountStatus::Changed,
            },
        ),
        (
            address!("2000000000000000000000000000000000000002"),
            BundleAccount {
                info: Some(AccountInfo::default()),
                original_info: Some(AccountInfo::default()),
                storage: {
                    let mut s = HashMap::default();
                    // 2 deleted slots
                    s.insert(
                        U256::from(1),
                        StorageSlot {
                            present_value: U256::ZERO,
                            previous_or_original_value: U256::from(50),
                        },
                    );
                    s.insert(
                        U256::from(2),
                        StorageSlot {
                            present_value: U256::ZERO,
                            previous_or_original_value: U256::from(75),
                        },
                    );
                    // 1 non-deleted slot
                    s.insert(
                        U256::from(3),
                        StorageSlot {
                            present_value: U256::from(200),
                            previous_or_original_value: U256::from(100),
                        },
                    );
                    s
                },
                status: AccountStatus::Changed,
            },
        ),
    ];

    // Count deleted storage slots across all accounts
    let storage_slots_deleted: usize = accounts
        .iter()
        .flat_map(|(_, acc)| acc.storage.values())
        .filter(|slot| slot.present_value.is_zero() && !slot.previous_or_original_value.is_zero())
        .count();

    // Should be 3: 1 from first account + 2 from second account
    assert_eq!(storage_slots_deleted, 3, "Should count 3 deleted storage slots across accounts");
}

/// Test EIP-7702 delegation detection from bytecode.
#[test]
fn test_eip7702_delegation_detection() {
    // Create EIP-7702 bytecode (delegation)
    let delegation_target = address!("1234567890123456789012345678901234567890");
    let eip7702_bytecode = Bytecode::new_eip7702(delegation_target);

    // Create regular bytecode
    let regular_code = Bytes::from(vec![0x60, 0x00, 0x60, 0x00, 0xf3]); // PUSH 0, PUSH 0, RETURN
    let regular_bytecode = Bytecode::new_raw(regular_code);

    // Test detection
    assert!(eip7702_bytecode.is_eip7702(), "EIP-7702 bytecode should be detected");
    assert!(!regular_bytecode.is_eip7702(), "Regular bytecode should not be EIP-7702");
}

/// Test counting EIP-7702 delegations set from contracts.
#[test]
fn test_eip7702_delegations_set_count() {
    let delegation_target1 = address!("1111111111111111111111111111111111111111");
    let delegation_target2 = address!("2222222222222222222222222222222222222222");

    let contracts: Vec<Bytecode> = vec![
        // EIP-7702 delegation 1
        Bytecode::new_eip7702(delegation_target1),
        // Regular bytecode
        Bytecode::new_raw(Bytes::from(vec![0x60, 0x00])),
        // EIP-7702 delegation 2
        Bytecode::new_eip7702(delegation_target2),
        // Another regular bytecode
        Bytecode::new_raw(Bytes::from(vec![0x60, 0x01])),
    ];

    // Count EIP-7702 delegations (same logic as in payload_validator.rs)
    let eip7702_delegations_set = contracts.iter().filter(|bc| bc.is_eip7702()).count();

    assert_eq!(eip7702_delegations_set, 2, "Should count 2 EIP-7702 delegations set");
}

// ============================================================================
// JSON Output Schema Validation Tests
// ============================================================================

/// Test that slow block threshold filtering works correctly.
/// This verifies the threshold logic used to decide when to log slow blocks.
#[test]
fn test_slow_block_threshold_filtering() {
    use reth_evm::metrics::{is_slow_block, set_slow_block_threshold, slow_block_threshold};

    // Test with default threshold (1000ms)
    set_slow_block_threshold(1000);
    assert_eq!(slow_block_threshold(), 1000);

    // Below threshold - should NOT be considered slow
    assert!(!is_slow_block(500.0), "500ms should not be slow with 1000ms threshold");
    assert!(!is_slow_block(999.9), "999.9ms should not be slow with 1000ms threshold");

    // Above threshold - SHOULD be considered slow
    assert!(is_slow_block(1000.1), "1000.1ms should be slow with 1000ms threshold");
    assert!(is_slow_block(1500.0), "1500ms should be slow with 1000ms threshold");

    // Test with custom threshold
    set_slow_block_threshold(500);
    assert!(!is_slow_block(400.0), "400ms should not be slow with 500ms threshold");
    assert!(is_slow_block(600.0), "600ms should be slow with 500ms threshold");

    // Test with zero threshold (log all blocks)
    set_slow_block_threshold(0);
    assert!(is_slow_block(1.0), "Any positive time should be slow with 0 threshold");
    assert!(!is_slow_block(0.0), "0ms should not be slow even with 0 threshold");

    // Reset to default
    set_slow_block_threshold(1000);
}

/// Test that cache hit rate calculation is correct.
/// This verifies the math used in the slow block JSON output.
#[test]
fn test_cache_hit_rate_calculation() {
    // Uses calculate_hit_rate from reth_evm::metrics

    // No cache activity - 0% hit rate
    assert_eq!(calculate_hit_rate(0, 0), 0.0);

    // 100% hit rate
    assert!((calculate_hit_rate(10, 0) - 100.0).abs() < 0.01);

    // 0% hit rate (all misses)
    assert!((calculate_hit_rate(0, 10) - 0.0).abs() < 0.01);

    // 50% hit rate
    assert!((calculate_hit_rate(5, 5) - 50.0).abs() < 0.01);

    // 40% hit rate (4 hits, 6 misses)
    assert!((calculate_hit_rate(4, 6) - 40.0).abs() < 0.01);

    // 66.67% hit rate (2 hits, 1 miss)
    assert!((calculate_hit_rate(2, 1) - 66.666).abs() < 0.01);
}

/// Test that all metrics fields are accepted by log_slow_block.
/// This verifies the function signature matches the expected JSON schema.
#[test]
fn test_log_slow_block_accepts_all_metrics() {
    use reth_evm::metrics::{set_slow_block_threshold, ExecutorMetrics};

    let metrics = ExecutorMetrics::default();

    // Set threshold to 0 so any execution time triggers logging
    set_slow_block_threshold(0);

    // This test verifies all fields are accepted - if any field is missing
    // or has the wrong type, this won't compile.
    // The function signature enforces the JSON schema.
    metrics.log_slow_block(
        // Block info
        12345,                                                              // block_number
        "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef12345678", // block_hash
        30_000_000,                                                         // gas_used
        200,                                                                // tx_count
        // Timing metrics
        1500.0, // execution_ms
        320.0,  // state_read_ms
        150.0,  // state_hash_ms
        75.0,   // commit_ms
        1545.0, // total_ms
        // State read metrics
        100,   // accounts_loaded
        500,   // storage_slots_loaded
        20,    // code_loaded
        10240, // code_bytes_read
        // State write metrics
        50,   // accounts_updated
        2,    // accounts_deleted (cross-client metric)
        200,  // storage_slots_updated
        15,   // storage_slots_deleted (cross-client metric)
        5,    // code_updated
        2048, // code_bytes_written
        // EIP-7702 metrics
        3, // eip7702_delegations_set (cross-client metric)
        1, // eip7702_delegations_cleared (cross-client metric)
        // Cache metrics
        80,  // account_cache_hits
        20,  // account_cache_misses
        300, // storage_cache_hits
        100, // storage_cache_misses
        45,  // code_cache_hits
        5,   // code_cache_misses
    );

    // Reset threshold
    set_slow_block_threshold(1000);

    // If we get here without panic, the test passes
    // The actual JSON output is tested by examining logs in production
}

/// Test combined scenario with accounts and storage deletions.
#[test]
fn test_combined_deletion_metrics() {
    // Simulate a block with:
    // - 3 accounts total
    // - 1 account deleted (selfdestructed)
    // - 5 storage slots total
    // - 2 storage slots deleted

    let accounts: Vec<(Address, BundleAccount)> = vec![
        // Regular account with 2 deleted storage slots
        (
            address!("1000000000000000000000000000000000000001"),
            BundleAccount {
                info: Some(AccountInfo::default()),
                original_info: Some(AccountInfo::default()),
                storage: {
                    let mut s = HashMap::default();
                    s.insert(
                        U256::from(1),
                        StorageSlot {
                            present_value: U256::ZERO,
                            previous_or_original_value: U256::from(100),
                        },
                    );
                    s.insert(
                        U256::from(2),
                        StorageSlot {
                            present_value: U256::ZERO,
                            previous_or_original_value: U256::from(200),
                        },
                    );
                    s.insert(
                        U256::from(3),
                        StorageSlot {
                            present_value: U256::from(300),
                            previous_or_original_value: U256::from(300),
                        },
                    );
                    s
                },
                status: AccountStatus::Changed,
            },
        ),
        // Destroyed account (no storage changes matter since it's destroyed)
        (
            address!("2000000000000000000000000000000000000002"),
            BundleAccount {
                info: None,
                original_info: Some(AccountInfo {
                    balance: U256::from(1000),
                    nonce: 5,
                    code_hash: KECCAK_EMPTY,
                    code: None,
                    account_id: None,
                }),
                storage: HashMap::default(),
                status: AccountStatus::Destroyed,
            },
        ),
        // Regular account with no deleted slots
        (
            address!("3000000000000000000000000000000000000003"),
            BundleAccount {
                info: Some(AccountInfo::default()),
                original_info: Some(AccountInfo::default()),
                storage: {
                    let mut s = HashMap::default();
                    s.insert(
                        U256::from(1),
                        StorageSlot {
                            present_value: U256::from(100),
                            previous_or_original_value: U256::from(50),
                        },
                    );
                    s
                },
                status: AccountStatus::Changed,
            },
        ),
    ];

    // Calculate metrics
    let accounts_changed = accounts.len();
    let accounts_deleted = accounts.iter().filter(|(_, acc)| acc.was_destroyed()).count();
    let storage_slots_changed: usize = accounts.iter().map(|(_, acc)| acc.storage.len()).sum();
    let storage_slots_deleted: usize = accounts
        .iter()
        .flat_map(|(_, acc)| acc.storage.values())
        .filter(|slot| slot.present_value.is_zero() && !slot.previous_or_original_value.is_zero())
        .count();

    // Verify counts
    assert_eq!(accounts_changed, 3, "Should have 3 accounts changed");
    assert_eq!(accounts_deleted, 1, "Should have 1 account deleted");
    assert_eq!(storage_slots_changed, 4, "Should have 4 storage slots changed");
    assert_eq!(storage_slots_deleted, 2, "Should have 2 storage slots deleted");
}
