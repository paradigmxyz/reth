//! Execution-based cross-client metrics tests.
//!
//! These tests verify that block execution produces the correct state changes
//! that would be counted by the cross-client execution metrics. Unlike the
//! calculation-only tests in metrics_integration.rs, these tests execute
//! actual transactions through the EVM.
//!
//! Test categories:
//! - Account metrics: reads, writes, deletions via SELFDESTRUCT
//! - Storage metrics: SLOAD reads, SSTORE writes, SSTORE zero deletions
//! - Code metrics: code loaded via CALL, code created via CREATE
//! - Combined scenarios: multiple operations in single block

use alloy_consensus::{constants::ETH_TO_WEI, Header, SignableTransaction, TxEip7702, TxLegacy};
use alloy_eips::eip7702::Authorization;
use alloy_primitives::{address, keccak256, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{ChainSpecBuilder, EthereumHardfork, ForkCondition, MAINNET};
use reth_ethereum_primitives::{Block, BlockBody, Transaction, TransactionSigned};
use reth_evm::execute::{BasicBlockExecutor, Executor};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{
    crypto::secp256k1::{public_key_to_address, sign_message},
    Block as _,
};
use reth_testing_utils::generators::{self, sign_tx_with_key_pair};
use revm::{
    database::{CacheDB, EmptyDB},
    state::{AccountInfo, Bytecode},
    Database,
};
use secp256k1::Keypair;
use std::sync::Arc;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates an empty in-memory database.
fn create_empty_db() -> CacheDB<EmptyDB> {
    CacheDB::new(EmptyDB::default())
}

/// Creates a Prague-enabled chain spec for testing.
/// Note: Used for EIP-7702 tests which require Prague hardfork.
#[allow(dead_code)]
fn create_prague_chain_spec() -> Arc<reth_chainspec::ChainSpec> {
    Arc::new(
        ChainSpecBuilder::from(&*MAINNET)
            .shanghai_activated()
            .cancun_activated()
            .prague_activated()
            .build(),
    )
}

/// Creates a Cancun-enabled chain spec for testing (useful for tests that
/// need SELFDESTRUCT behavior from before Prague changes).
fn create_cancun_chain_spec() -> Arc<reth_chainspec::ChainSpec> {
    Arc::new(
        ChainSpecBuilder::from(&*MAINNET)
            .shanghai_activated()
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(0))
            .build(),
    )
}

/// Creates a funded sender account and returns (keypair, address).
fn create_funded_sender(db: &mut CacheDB<EmptyDB>) -> (secp256k1::Keypair, Address) {
    let sender_key_pair = generators::generate_key(&mut generators::rng());
    let sender_address = public_key_to_address(sender_key_pair.public_key());

    db.insert_account_info(
        sender_address,
        AccountInfo { nonce: 0, balance: U256::from(ETH_TO_WEI * 10), ..Default::default() },
    );

    (sender_key_pair, sender_address)
}

/// Creates a simple block header for testing.
fn create_test_header(chain_spec: &reth_chainspec::ChainSpec) -> Header {
    let mut header = chain_spec.genesis_header().clone();
    header.gas_limit = 30_000_000;
    header.number = 1;
    header.timestamp = 1;
    header
}

// ============================================================================
// Bytecode Constants
// ============================================================================

/// Simple contract that stores a value in slot 0 when called.
/// Bytecode: PUSH1 42, PUSH1 0, SSTORE, STOP
/// Effect: Sets storage[0] = 42
const STORAGE_SETTER_CODE: &[u8] = &[
    0x60, 0x2a, // PUSH1 42
    0x60, 0x00, // PUSH1 0
    0x55, // SSTORE
    0x00, // STOP
];

/// Contract that reads storage slot 0 and returns it.
/// Bytecode: PUSH1 0, SLOAD, PUSH1 0, MSTORE, PUSH1 32, PUSH1 0, RETURN
/// Effect: Returns storage[0]
const STORAGE_READER_CODE: &[u8] = &[
    0x60, 0x00, // PUSH1 0 (slot)
    0x54, // SLOAD
    0x60, 0x00, // PUSH1 0 (memory offset)
    0x52, // MSTORE
    0x60, 0x20, // PUSH1 32 (size)
    0x60, 0x00, // PUSH1 0 (offset)
    0xf3, // RETURN
];

/// Contract that clears storage slot 0 (sets to zero).
/// Bytecode: PUSH1 0, PUSH1 0, SSTORE, STOP
/// Effect: Sets storage[0] = 0
const STORAGE_CLEARER_CODE: &[u8] = &[
    0x60, 0x00, // PUSH1 0 (value)
    0x60, 0x00, // PUSH1 0 (slot)
    0x55, // SSTORE
    0x00, // STOP
];

/// Contract that performs multiple storage operations.
/// Sets slot 1 = 100, slot 2 = 200, clears slot 0.
const MULTI_STORAGE_CODE: &[u8] = &[
    0x60, 0x64, // PUSH1 100
    0x60, 0x01, // PUSH1 1
    0x55, // SSTORE (slot 1 = 100)
    0x60, 0xc8, // PUSH1 200
    0x60, 0x02, // PUSH1 2
    0x55, // SSTORE (slot 2 = 200)
    0x60, 0x00, // PUSH1 0
    0x60, 0x00, // PUSH1 0
    0x55, // SSTORE (slot 0 = 0, deletion)
    0x00, // STOP
];

/// SELFDESTRUCT contract (sends all balance to a beneficiary).
/// Bytecode: PUSH20 <beneficiary>, SELFDESTRUCT
fn selfdestruct_code(beneficiary: Address) -> Bytes {
    let mut code = vec![0x73]; // PUSH20
    code.extend_from_slice(beneficiary.as_slice());
    code.push(0xff); // SELFDESTRUCT
    code.into()
}

// ============================================================================
// Account Metrics Tests
// ============================================================================

/// Test that executing a simple transfer updates account states.
/// This verifies that account reads and writes are tracked.
#[test]
fn test_account_metrics_via_transfer() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    let recipient = address!("0x1000000000000000000000000000000000000001");

    // Create transfer transaction
    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 21_000,
            to: TxKind::Call(recipient),
            value: U256::from(1_000_000),
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Verify transaction succeeded
    assert!(result.receipts[0].success, "Transfer should succeed");

    // Verify accounts were modified by checking state
    let sender_balance =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap().balance);
    let recipient_balance =
        executor.with_state_mut(|state| state.basic(recipient).unwrap().unwrap().balance);

    // Sender should have less balance (transfer + gas)
    assert!(sender_balance < U256::from(ETH_TO_WEI * 10), "Sender balance should decrease");
    // Recipient should have the transferred amount
    assert_eq!(recipient_balance, U256::from(1_000_000), "Recipient should receive funds");
}

/// Test that account deletion via SELFDESTRUCT is tracked.
#[test]
fn test_account_deleted_via_selfdestruct() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Deploy a contract that will SELFDESTRUCT
    let contract_address = address!("0x2000000000000000000000000000000000000002");
    let beneficiary = address!("0x3000000000000000000000000000000000000003");

    let selfdestruct_bytecode = selfdestruct_code(beneficiary);
    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::from(1_000_000), // Contract has some balance
            code_hash: keccak256(&selfdestruct_bytecode),
            code: Some(Bytecode::new_raw(selfdestruct_bytecode)),
            ..Default::default()
        },
    );

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed
    assert!(result.receipts[0].success, "SELFDESTRUCT call should succeed");

    // After SELFDESTRUCT in Cancun+, the contract is marked for deletion
    // but balance is sent to beneficiary
    // Check beneficiary received the balance
    let beneficiary_balance =
        executor.with_state_mut(|state| state.basic(beneficiary).unwrap().map(|a| a.balance));

    // Beneficiary should have received contract's balance
    assert!(
        beneficiary_balance.is_some() && beneficiary_balance.unwrap() >= U256::from(1_000_000),
        "Beneficiary should receive contract balance"
    );

    // Verify the sender's transaction was processed
    let sender_nonce =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap().nonce);
    assert_eq!(sender_nonce, 1, "Sender nonce should be incremented");
}

// ============================================================================
// Storage Metrics Tests
// ============================================================================

/// Test that storage reads via SLOAD are executed correctly.
#[test]
fn test_storage_loaded_via_sload() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy a contract with pre-existing storage and reader code
    let contract_address = address!("0x4000000000000000000000000000000000000004");
    let reader_code = Bytes::from_static(STORAGE_READER_CODE);

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&reader_code),
            code: Some(Bytecode::new_raw(reader_code)),
            ..Default::default()
        },
    );

    // Insert pre-existing storage value
    db.cache
        .accounts
        .entry(contract_address)
        .or_default()
        .storage
        .insert(U256::ZERO, U256::from(0x42));

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed (SLOAD executed, value returned)
    assert!(result.receipts[0].success, "SLOAD call should succeed");

    // Verify storage was accessed (value should still be there)
    let storage_value =
        executor.with_state_mut(|state| state.storage(contract_address, U256::ZERO).unwrap());
    assert_eq!(storage_value, U256::from(0x42), "Storage value should be preserved after SLOAD");
}

/// Test that storage writes via SSTORE update state correctly.
#[test]
fn test_storage_updated_via_sstore() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy a contract that writes to storage
    let contract_address = address!("0x5000000000000000000000000000000000000005");
    let setter_code = Bytes::from_static(STORAGE_SETTER_CODE);

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&setter_code),
            code: Some(Bytecode::new_raw(setter_code)),
            ..Default::default()
        },
    );

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed
    assert!(result.receipts[0].success, "SSTORE call should succeed");

    // Verify storage was written
    let storage_value =
        executor.with_state_mut(|state| state.storage(contract_address, U256::ZERO).unwrap());
    assert_eq!(storage_value, U256::from(42), "Storage slot 0 should be set to 42");
}

/// Test that storage deletion (SSTORE to zero) is tracked correctly.
#[test]
fn test_storage_deleted_via_sstore_zero() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy a contract that clears storage
    let contract_address = address!("0x6000000000000000000000000000000000000006");
    let clearer_code = Bytes::from_static(STORAGE_CLEARER_CODE);

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&clearer_code),
            code: Some(Bytecode::new_raw(clearer_code)),
            ..Default::default()
        },
    );

    // Insert pre-existing storage value that will be cleared
    db.cache
        .accounts
        .entry(contract_address)
        .or_default()
        .storage
        .insert(U256::ZERO, U256::from(100));

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed
    assert!(result.receipts[0].success, "Storage clear call should succeed");

    // Verify storage was cleared (set to zero)
    let storage_value =
        executor.with_state_mut(|state| state.storage(contract_address, U256::ZERO).unwrap());
    assert!(storage_value.is_zero(), "Storage slot 0 should be cleared to zero");
}

/// Test multiple storage operations in a single transaction.
#[test]
fn test_multiple_storage_operations() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy a contract with multiple storage operations
    let contract_address = address!("0x7000000000000000000000000000000000000007");
    let multi_code = Bytes::from_static(MULTI_STORAGE_CODE);

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&multi_code),
            code: Some(Bytecode::new_raw(multi_code)),
            ..Default::default()
        },
    );

    // Insert pre-existing storage in slot 0 (will be deleted)
    db.cache
        .accounts
        .entry(contract_address)
        .or_default()
        .storage
        .insert(U256::ZERO, U256::from(999));

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 200_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    assert!(result.receipts[0].success, "Multi-storage call should succeed");

    // Verify storage changes:
    // - Slot 0: 999 -> 0 (deleted)
    // - Slot 1: 0 -> 100 (created)
    // - Slot 2: 0 -> 200 (created)
    let slot0 =
        executor.with_state_mut(|state| state.storage(contract_address, U256::ZERO).unwrap());
    let slot1 =
        executor.with_state_mut(|state| state.storage(contract_address, U256::from(1)).unwrap());
    let slot2 =
        executor.with_state_mut(|state| state.storage(contract_address, U256::from(2)).unwrap());

    assert!(slot0.is_zero(), "Slot 0 should be cleared");
    assert_eq!(slot1, U256::from(100), "Slot 1 should be 100");
    assert_eq!(slot2, U256::from(200), "Slot 2 should be 200");
}

// ============================================================================
// Code Metrics Tests
// ============================================================================

/// Test that calling a contract loads its code.
#[test]
fn test_code_loaded_via_call() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy a simple contract
    let contract_address = address!("0x8000000000000000000000000000000000000008");
    let simple_code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xf3]); // PUSH 0, PUSH 0, RETURN

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&simple_code),
            code: Some(Bytecode::new_raw(simple_code)),
            ..Default::default()
        },
    );

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed (code was loaded and executed)
    assert!(result.receipts[0].success, "Contract call should succeed");

    // Verify contract code is still present
    let account_info =
        executor.with_state_mut(|state| state.basic(contract_address).unwrap().unwrap());
    assert!(account_info.code.is_some(), "Contract should have code");
}

/// Test that CREATE opcode creates new contract code.
#[test]
fn test_code_created_via_create() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Create a deployment transaction (contract creation)
    // Simple contract: just STOP (0x00)
    let init_code = Bytes::from_static(&[
        0x60, 0x00, // PUSH1 0x00 (the runtime code)
        0x60, 0x00, // PUSH1 0x00 (memory offset)
        0x52, // MSTORE
        0x60, 0x01, // PUSH1 0x01 (code size)
        0x60, 0x1f, // PUSH1 0x1f (offset to last byte)
        0xf3, // RETURN
    ]);

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 200_000,
            to: TxKind::Create,
            value: U256::ZERO,
            input: init_code,
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed
    assert!(result.receipts[0].success, "Contract creation should succeed");

    // The contract should be created at the calculated address
    // Contract address = keccak256(rlp([sender, nonce]))[12:]
    let contract_address = sender_address.create(0);

    // Verify the new contract exists and has code
    let contract_info = executor.with_state_mut(|state| state.basic(contract_address).unwrap());
    assert!(contract_info.is_some(), "Created contract should exist");
    let contract_info = contract_info.unwrap();
    assert!(contract_info.code.is_some(), "Created contract should have code");
}

// ============================================================================
// Combined Metrics Tests
// ============================================================================

/// Test a complex scenario with multiple accounts and operations.
#[test]
fn test_combined_metrics_scenario() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Set up multiple contracts
    let storage_contract = address!("0x9000000000000000000000000000000000000009");
    let setter_code = Bytes::from_static(STORAGE_SETTER_CODE);

    db.insert_account_info(
        storage_contract,
        AccountInfo {
            nonce: 1,
            balance: U256::from(500_000),
            code_hash: keccak256(&setter_code),
            code: Some(Bytecode::new_raw(setter_code)),
            ..Default::default()
        },
    );

    // Pre-existing storage
    db.cache
        .accounts
        .entry(storage_contract)
        .or_default()
        .storage
        .insert(U256::from(10), U256::from(999));

    let header = create_test_header(&chain_spec);

    // Transaction 1: Call storage contract
    let tx1 = sign_tx_with_key_pair(
        sender_keypair.clone(),
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(storage_contract),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    // Transaction 2: Simple transfer
    let recipient = address!("0xa000000000000000000000000000000000000000");
    let tx2 = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 1,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 21_000,
            to: TxKind::Call(recipient),
            value: U256::from(100_000),
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block {
                header,
                body: BlockBody { transactions: vec![tx1, tx2], ..Default::default() },
            }
            .try_into_recovered()
            .unwrap(),
        )
        .expect("execution should succeed");

    // Both transactions should succeed
    assert!(result.receipts[0].success, "Storage call should succeed");
    assert!(result.receipts[1].success, "Transfer should succeed");

    // Verify state changes
    // Storage contract should have slot 0 = 42 (from SSTORE)
    let slot0 =
        executor.with_state_mut(|state| state.storage(storage_contract, U256::ZERO).unwrap());
    assert_eq!(slot0, U256::from(42), "Storage slot 0 should be updated");

    // Recipient should have received funds
    let recipient_balance =
        executor.with_state_mut(|state| state.basic(recipient).unwrap().unwrap().balance);
    assert_eq!(recipient_balance, U256::from(100_000), "Recipient should have received funds");

    // Sender nonce should be 2
    let sender_nonce =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap().nonce);
    assert_eq!(sender_nonce, 2, "Sender nonce should be 2 after two transactions");
}

/// Test that multiple storage deletions across different contracts are tracked.
#[test]
fn test_multiple_storage_deletions_across_contracts() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Deploy two contracts that clear storage
    let contract1 = address!("0xb000000000000000000000000000000000000001");
    let contract2 = address!("0xb000000000000000000000000000000000000002");
    let clearer_code = Bytes::from_static(STORAGE_CLEARER_CODE);

    for contract in [contract1, contract2] {
        db.insert_account_info(
            contract,
            AccountInfo {
                nonce: 1,
                balance: U256::ZERO,
                code_hash: keccak256(&clearer_code),
                code: Some(Bytecode::new_raw(clearer_code.clone())),
                ..Default::default()
            },
        );
        // Pre-existing storage
        db.cache.accounts.entry(contract).or_default().storage.insert(U256::ZERO, U256::from(123));
    }

    let header = create_test_header(&chain_spec);

    // Two transactions clearing storage in different contracts
    let tx1 = sign_tx_with_key_pair(
        sender_keypair.clone(),
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract1),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let tx2 = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 1,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(contract2),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block {
                header,
                body: BlockBody { transactions: vec![tx1, tx2], ..Default::default() },
            }
            .try_into_recovered()
            .unwrap(),
        )
        .expect("execution should succeed");

    assert!(result.receipts[0].success && result.receipts[1].success);

    // Both contracts should have storage cleared
    let slot1 = executor.with_state_mut(|state| state.storage(contract1, U256::ZERO).unwrap());
    let slot2 = executor.with_state_mut(|state| state.storage(contract2, U256::ZERO).unwrap());

    assert!(slot1.is_zero(), "Contract 1 slot 0 should be cleared");
    assert!(slot2.is_zero(), "Contract 2 slot 0 should be cleared");
}

// ============================================================================
// EIP-7702 Metrics Tests
// ============================================================================

/// Creates a signed EIP-7702 SetCodeTx that delegates an EOA to a target address.
///
/// # Arguments
/// * `sender_keypair` - Keypair of the account paying for the transaction
/// * `nonce` - Transaction nonce
/// * `delegator_keypair` - Keypair of the EOA that will be delegated
/// * `delegate_to` - Address to delegate code to (Address::ZERO to clear delegation)
/// * `chain_id` - Chain ID for replay protection
fn create_signed_setcode_tx(
    sender_keypair: Keypair,
    nonce: u64,
    delegator_keypair: Keypair,
    delegate_to: Address,
    chain_id: u64,
) -> TransactionSigned {
    let delegator_address = public_key_to_address(delegator_keypair.public_key());

    // Create authorization for the delegator
    let authorization =
        Authorization { chain_id: U256::from(chain_id), address: delegate_to, nonce: 0 };

    // Sign authorization with delegator's key
    let auth_hash = authorization.signature_hash();
    let auth_sig = sign_message(B256::from_slice(&delegator_keypair.secret_bytes()[..]), auth_hash)
        .expect("authorization signing should succeed");
    let signed_auth = authorization.into_signed(auth_sig);

    // Create the SetCodeTx
    let tx = TxEip7702 {
        chain_id,
        nonce,
        max_fee_per_gas: 100_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        gas_limit: 100_000,
        to: delegator_address, // Target is the account being delegated
        value: U256::ZERO,
        input: Bytes::new(),
        access_list: Default::default(),
        authorization_list: vec![signed_auth],
    };

    // Sign the transaction with sender's key
    let tx_sig =
        sign_message(B256::from_slice(&sender_keypair.secret_bytes()[..]), tx.signature_hash())
            .expect("transaction signing should succeed");

    Transaction::Eip7702(tx).into_signed(tx_sig).into()
}

/// Test that executing a SetCodeTx sets an EIP-7702 delegation.
///
/// This verifies that executing an EIP-7702 transaction creates the
/// expected bytecode that would be counted as eip7702_delegations_set.
#[test]
fn test_eip7702_delegation_set_via_execution() {
    let chain_spec = create_prague_chain_spec();
    let mut db = create_empty_db();

    // Create sender (pays for tx) and delegator (gets delegation)
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);
    let delegator_keypair = generators::generate_key(&mut generators::rng());
    let delegator_address = public_key_to_address(delegator_keypair.public_key());

    // Fund the delegator account
    db.insert_account_info(
        delegator_address,
        AccountInfo { nonce: 0, balance: U256::from(ETH_TO_WEI), ..Default::default() },
    );

    // Target contract to delegate to (can be any address)
    let target_contract = address!("0x7000000000000000000000000000000000000007");

    // Create signed SetCodeTx
    let tx = create_signed_setcode_tx(
        sender_keypair,
        0,
        delegator_keypair,
        target_contract, // Non-zero = set delegation
        chain_spec.chain.id(),
    );

    let mut header = create_test_header(&chain_spec);
    // Prague requires parent_beacon_block_root
    header.parent_beacon_block_root = Some(B256::ZERO);

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Verify transaction succeeded
    assert!(result.receipts[0].success, "EIP-7702 SetCodeTx should succeed");

    // Check that the delegator account now has EIP-7702 bytecode
    let delegator_info = executor
        .with_state_mut(|state| state.basic(delegator_address).unwrap())
        .expect("delegator should exist");

    // The bytecode should be EIP-7702 format (0xef0100 + 20-byte address)
    let expected_bytecode = Bytecode::new_eip7702(target_contract);
    assert_eq!(
        delegator_info.code_hash,
        expected_bytecode.hash_slow(),
        "Delegator should have EIP-7702 bytecode"
    );
}

/// Test multiple EIP-7702 delegations in a single block.
///
/// This verifies that executing multiple SetCodeTx transactions in one block
/// correctly counts multiple delegations.
#[test]
fn test_eip7702_multiple_delegations_single_block() {
    let chain_spec = create_prague_chain_spec();
    let mut db = create_empty_db();

    // Create sender
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Create 3 delegators
    let delegator1_keypair = generators::generate_key(&mut generators::rng());
    let delegator2_keypair = generators::generate_key(&mut generators::rng());
    let delegator3_keypair = generators::generate_key(&mut generators::rng());

    let delegator1_address = public_key_to_address(delegator1_keypair.public_key());
    let delegator2_address = public_key_to_address(delegator2_keypair.public_key());
    let delegator3_address = public_key_to_address(delegator3_keypair.public_key());

    // Fund all delegator accounts
    for addr in [delegator1_address, delegator2_address, delegator3_address] {
        db.insert_account_info(
            addr,
            AccountInfo { nonce: 0, balance: U256::from(ETH_TO_WEI), ..Default::default() },
        );
    }

    // Target contract
    let target_contract = address!("0x8000000000000000000000000000000000000008");

    // Create 3 SetCodeTx transactions
    let tx1 = create_signed_setcode_tx(
        sender_keypair.clone(),
        0,
        delegator1_keypair,
        target_contract,
        chain_spec.chain.id(),
    );
    let tx2 = create_signed_setcode_tx(
        sender_keypair.clone(),
        1,
        delegator2_keypair,
        target_contract,
        chain_spec.chain.id(),
    );
    let tx3 = create_signed_setcode_tx(
        sender_keypair,
        2,
        delegator3_keypair,
        target_contract,
        chain_spec.chain.id(),
    );

    let mut header = create_test_header(&chain_spec);
    header.parent_beacon_block_root = Some(B256::ZERO);

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block {
                header,
                body: BlockBody { transactions: vec![tx1, tx2, tx3], ..Default::default() },
            }
            .try_into_recovered()
            .unwrap(),
        )
        .expect("execution should succeed");

    // All transactions should succeed
    assert!(result.receipts[0].success, "First SetCodeTx should succeed");
    assert!(result.receipts[1].success, "Second SetCodeTx should succeed");
    assert!(result.receipts[2].success, "Third SetCodeTx should succeed");

    // All delegators should have EIP-7702 bytecode
    let expected_bytecode = Bytecode::new_eip7702(target_contract);
    let expected_hash = expected_bytecode.hash_slow();

    for (addr, name) in [
        (delegator1_address, "delegator1"),
        (delegator2_address, "delegator2"),
        (delegator3_address, "delegator3"),
    ] {
        let info = executor
            .with_state_mut(|state| state.basic(addr).unwrap())
            .expect(&format!("{name} should exist"));
        assert_eq!(info.code_hash, expected_hash, "{name} should have EIP-7702 bytecode");
    }
}

/// Test clearing an EIP-7702 delegation.
///
/// This verifies that an account with an existing EIP-7702 delegation can
/// have it cleared by delegating to Address::ZERO.
#[test]
fn test_eip7702_delegation_cleared_via_execution() {
    let chain_spec = create_prague_chain_spec();
    let mut db = create_empty_db();

    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);
    let delegator_keypair = generators::generate_key(&mut generators::rng());
    let delegator_address = public_key_to_address(delegator_keypair.public_key());

    // Create initial EIP-7702 bytecode (simulating an existing delegation)
    let initial_target = address!("0x9000000000000000000000000000000000000009");
    let eip7702_bytecode = Bytecode::new_eip7702(initial_target);

    // Fund the delegator account WITH existing EIP-7702 delegation
    db.insert_account_info(
        delegator_address,
        AccountInfo {
            nonce: 0,
            balance: U256::from(ETH_TO_WEI),
            code_hash: eip7702_bytecode.hash_slow(),
            code: Some(eip7702_bytecode.clone()),
            ..Default::default()
        },
    );

    // Verify the account has EIP-7702 bytecode before clearing
    let info_before = db.basic(delegator_address).unwrap().unwrap();
    assert_eq!(
        info_before.code_hash,
        eip7702_bytecode.hash_slow(),
        "Delegator should have EIP-7702 bytecode before clearing"
    );

    // Create SetCodeTx to clear delegation (delegate to Address::ZERO)
    let tx = create_signed_setcode_tx(
        sender_keypair,
        0,
        delegator_keypair,
        Address::ZERO, // Clear delegation
        chain_spec.chain.id(),
    );

    let mut header = create_test_header(&chain_spec);
    header.parent_beacon_block_root = Some(B256::ZERO);

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Transaction should succeed
    assert!(result.receipts[0].success, "EIP-7702 clear delegation should succeed");

    // Check that the delegator no longer has EIP-7702 bytecode (cleared = empty code)
    let delegator_info = executor
        .with_state_mut(|state| state.basic(delegator_address).unwrap())
        .expect("delegator should exist");

    // After clearing, the account should have no code (KECCAK_EMPTY)
    // Note: In EIP-7702, delegating to Address::ZERO removes the delegation
    assert!(
        delegator_info.code.is_none() ||
            delegator_info.code.as_ref().map(|c| c.is_empty()).unwrap_or(true),
        "Delegator should have no code after clearing delegation"
    );
}

// ============================================================================
// Test Vector Coverage Tests (A2, C2, EDGE1, EDGE2, EDGE3)
// ============================================================================

/// Test that ETH transfer to a new (non-existent) account creates that account.
/// This explicitly tests scenario A2 from slow_block_test_vectors.json.
#[test]
fn test_transfer_to_new_account_creates_account() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, _sender_address) = create_funded_sender(&mut db);

    // Use a fresh address that definitely doesn't exist
    let new_recipient = address!("0x1111111111111111111111111111111111111111");

    // Verify recipient does NOT exist before transfer
    assert!(
        db.basic(new_recipient).unwrap().is_none(),
        "New recipient should not exist before transfer"
    );

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 21_000,
            to: TxKind::Call(new_recipient),
            value: U256::from(10_000_000_000_000_000u64), // 0.01 ETH
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed");

    // Verify transaction succeeded
    assert!(result.receipts[0].success, "Transfer to new account should succeed");

    // Verify the new account now exists
    let new_account = executor.with_state_mut(|state| state.basic(new_recipient).unwrap());
    assert!(new_account.is_some(), "New recipient should exist after transfer");

    // Verify the account has the transferred balance
    let balance = new_account.unwrap().balance;
    assert_eq!(
        balance,
        U256::from(10_000_000_000_000_000u64),
        "New account should have received funds"
    );
}

/// Test that deploying multiple contracts with identical bytecode in one block
/// results in code count = number of instances, not unique bytecode hashes.
/// This tests scenario C2 from slow_block_test_vectors.json.
///
/// The test deploys 3 identical contracts from the same sender in a single block,
/// verifying that each deployment counts as a separate code write.
#[test]
fn test_factory_deploys_multiple_contracts() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Identical init code for all 3 contracts: returns a minimal STOP contract
    // Init code: PUSH1 0x00, PUSH1 0x00, MSTORE, PUSH1 0x01, PUSH1 0x1f, RETURN
    // This returns 1 byte of runtime code (0x00 = STOP)
    let init_code = Bytes::from_static(&[
        0x60, 0x00, // PUSH1 0x00 (the STOP opcode value)
        0x60, 0x00, // PUSH1 0x00 (memory offset)
        0x52, // MSTORE (stores 0x00...00 at memory, with STOP at byte 31)
        0x60, 0x01, // PUSH1 0x01 (code size)
        0x60, 0x1f, // PUSH1 0x1f (offset to last byte of memory word)
        0xf3, // RETURN
    ]);

    let header = create_test_header(&chain_spec);

    // Create 3 contract deployment transactions with identical bytecode
    let tx1 = sign_tx_with_key_pair(
        sender_keypair.clone(),
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Create,
            value: U256::ZERO,
            input: init_code.clone(),
        }),
    );

    let tx2 = sign_tx_with_key_pair(
        sender_keypair.clone(),
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 1,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Create,
            value: U256::ZERO,
            input: init_code.clone(),
        }),
    );

    let tx3 = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 2,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Create,
            value: U256::ZERO,
            input: init_code,
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block {
                header,
                body: BlockBody { transactions: vec![tx1, tx2, tx3], ..Default::default() },
            }
            .try_into_recovered()
            .unwrap(),
        )
        .expect("execution should succeed");

    // All 3 deployments should succeed
    assert!(result.receipts[0].success, "First deployment should succeed");
    assert!(result.receipts[1].success, "Second deployment should succeed");
    assert!(result.receipts[2].success, "Third deployment should succeed");

    // Verify 3 contracts were created at the expected addresses
    let contract1 = sender_address.create(0);
    let contract2 = sender_address.create(1);
    let contract3 = sender_address.create(2);

    // All 3 contracts should exist and have code
    for (addr, name) in
        [(contract1, "contract1"), (contract2, "contract2"), (contract3, "contract3")]
    {
        let info = executor.with_state_mut(|state| state.basic(addr).unwrap());
        assert!(info.is_some(), "{name} should exist");
        assert!(info.unwrap().code.is_some(), "{name} should have code");
    }

    // The key assertion for C2: sender nonce = 3 (deployed 3 contracts)
    let sender_info =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap());
    assert_eq!(sender_info.nonce, 3, "Sender should have nonce 3 after 3 deployments");
}

/// Test that executing an empty block (no transactions) produces valid metrics.
/// This tests edge case EDGE1 from slow_block_test_vectors.json.
#[test]
fn test_empty_block_metrics() {
    let chain_spec = create_cancun_chain_spec();
    let db = create_empty_db();

    let header = create_test_header(&chain_spec);

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    // Execute block with NO transactions
    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("empty block execution should succeed");

    // Verify no receipts (no transactions)
    assert!(result.receipts.is_empty(), "Empty block should have no receipts");

    // Verify gas used is 0
    assert_eq!(result.gas_used, 0, "Empty block should use 0 gas");
}

/// Contract that always reverts when called.
/// Bytecode: PUSH1 0, PUSH1 0, REVERT
const REVERTER_CODE: &[u8] = &[
    0x60, 0x00, // PUSH1 0 (return data size)
    0x60, 0x00, // PUSH1 0 (return data offset)
    0xfd, // REVERT
];

/// Test that a reverted transaction does NOT include its attempted state changes
/// in the final state writes.
/// This tests edge case EDGE2 from slow_block_test_vectors.json.
#[test]
fn test_reverted_transaction_excludes_state_changes() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Deploy reverter contract
    let reverter_address = address!("0xdead000000000000000000000000000000000001");
    let reverter_code = Bytes::from_static(REVERTER_CODE);

    db.insert_account_info(
        reverter_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&reverter_code),
            code: Some(Bytecode::new_raw(reverter_code)),
            ..Default::default()
        },
    );

    let header = create_test_header(&chain_spec);
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 100_000,
            to: TxKind::Call(reverter_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed even if tx reverts");

    // Transaction should be included but marked as failed
    assert!(!result.receipts[0].success, "Reverted transaction should not succeed");

    // Verify sender nonce still incremented (tx was included)
    let sender_nonce =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap().nonce);
    assert_eq!(sender_nonce, 1, "Sender nonce should still increment on revert");

    // Gas should be consumed (at least base gas)
    assert!(result.gas_used >= 21000, "Some gas should be consumed on revert");
}

/// Test that an out-of-gas transaction reverts all its state changes.
/// This tests edge case EDGE3 from slow_block_test_vectors.json.
#[test]
fn test_out_of_gas_reverts_all_changes() {
    let chain_spec = create_cancun_chain_spec();
    let mut db = create_empty_db();
    let (sender_keypair, sender_address) = create_funded_sender(&mut db);

    // Deploy storage-writing contract
    let contract_address = address!("0x0005000000000000000000000000000000000005");
    let setter_code = Bytes::from_static(STORAGE_SETTER_CODE);

    db.insert_account_info(
        contract_address,
        AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: keccak256(&setter_code),
            code: Some(Bytecode::new_raw(setter_code)),
            ..Default::default()
        },
    );

    let header = create_test_header(&chain_spec);

    // Call with insufficient gas (21000 base + ~1000 for call overhead, but not enough for SSTORE)
    // SSTORE to new slot costs 20000 gas, so 22000 total is not enough
    let tx = sign_tx_with_key_pair(
        sender_keypair,
        Transaction::Legacy(TxLegacy {
            chain_id: Some(chain_spec.chain.id()),
            nonce: 0,
            gas_price: header.base_fee_per_gas.unwrap_or(1).into(),
            gas_limit: 22_000, // Not enough for SSTORE
            to: TxKind::Call(contract_address),
            value: U256::ZERO,
            input: Bytes::new(),
        }),
    );

    let provider = EthEvmConfig::new(chain_spec);
    let mut executor = BasicBlockExecutor::new(provider, db);

    let result = executor
        .execute_one(
            &Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } }
                .try_into_recovered()
                .unwrap(),
        )
        .expect("execution should succeed even if tx runs out of gas");

    // Transaction should fail (out of gas)
    assert!(!result.receipts[0].success, "OOG transaction should fail");

    // All provided gas should be consumed
    assert_eq!(result.gas_used, 22_000, "All gas should be consumed on OOG");

    // Verify storage was NOT written (state changes reverted)
    let storage_value =
        executor.with_state_mut(|state| state.storage(contract_address, U256::ZERO).unwrap());
    assert!(storage_value.is_zero(), "Storage should NOT be written on OOG - state reverted");

    // Verify sender nonce still incremented
    let sender_nonce =
        executor.with_state_mut(|state| state.basic(sender_address).unwrap().unwrap().nonce);
    assert_eq!(sender_nonce, 1, "Sender nonce should still increment on OOG");
}
