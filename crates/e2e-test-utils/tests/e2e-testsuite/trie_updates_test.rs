//! Test suite for validating trie updates in canonical blocks.
//!
//! This test verifies that trie updates are properly preserved when:
//! - Blocks are added to the canonical chain
//! - Storage values are modified in subsequent blocks
//!
//! The test specifically checks for a bug where blocks would have
//! missing trie updates when not connecting to the last persisted block.

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, TransactionBuilder};
use alloy_primitives::{address, Address, Bytes, TxKind, U256};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{AssertMissingTrieUpdates, CaptureBlock, ProduceBlockWithTransactionsViaEngineAPI},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_api::TreeConfig;
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use std::{str::FromStr, sync::Arc};

/// Storage contract address for our tests
const STORAGE_CONTRACT: Address = address!("1234567890123456789012345678901234567890");

/// Test account private key (from common test accounts)
const TEST_PRIVATE_KEY_1: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Storage slot configuration for trie update tests.
///
/// These slots are specifically chosen to create branch nodes in the Merkle Patricia Trie
/// by having common hash prefixes when hashed with keccak256.
struct StorageSlots {
    slot_a: U256, // Group 1: hashes to 0x70e0 prefix
    slot_b: U256, // Group 1: hashes to 0x70e0 prefix
    slot_c: U256, // Group 1: hashes to 0x70e0 prefix
    slot_d: U256, // Group 2: hashes to 0x05f3 prefix
}

/// Storage values for a single block
struct BlockStorageValues {
    slot_a: U256,
    slot_b: U256,
    slot_c: U256,
    slot_d: U256,
}

/// Storage values used for testing trie updates across blocks
struct StorageValues {
    /// Values to set in block 1
    block_1: BlockStorageValues,
    /// Values to set in block 2
    block_2: BlockStorageValues,
}

/// Test transactions for different blocks
struct TestTransactions {
    /// Transactions for Block 1 (sets initial storage values)
    set_initial_values: Vec<Bytes>,
    /// Transactions for Block 2 (modifies storage values)
    modify_values: Vec<Bytes>,
}

/// Test that trie updates are properly preserved in canonical blocks
#[tokio::test]
async fn test_trie_updates_preserved_in_canonical_blocks() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Initialize test configuration
    let setup = create_test_setup();
    let slots = init_storage_slots();
    let values = init_storage_values();
    let txs = create_test_transactions(&slots, &values).await?;

    let test = TestBuilder::new()
        .with_setup(setup)
        // Create block 1: Set initial storage values
        .with_action(ProduceBlockWithTransactionsViaEngineAPI::new(
            txs.set_initial_values,
            "block_1",
        ))
        .with_action(CaptureBlock::new("block_1"))
        // Verify block 1 has trie updates
        .with_action(AssertMissingTrieUpdates::new("block_1").expect_missing(false))
        // Create block 2: Modify storage values
        .with_action(ProduceBlockWithTransactionsViaEngineAPI::new(txs.modify_values, "block_2"))
        .with_action(CaptureBlock::new("block_2"))
        // Verify block 2 also has trie updates (this was failing before the fix)
        .with_action(AssertMissingTrieUpdates::new("block_2").expect_missing(false));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Creates a genesis state with:
/// - A storage contract with slots that create branch nodes in the trie
/// - Test accounts with funds
fn create_test_genesis() -> serde_json::Value {
    let alloc = serde_json::json!({
        format!("{:?}", STORAGE_CONTRACT): {
            "balance": "0x0",
            "code": "0x6000356020359055", // Simple storage contract: stores value at slot
            // Storage slots that create branch nodes by having common hash prefixes
            "storage": {
                // Group 1: slots that hash to prefix 0x70e0 (creates branch at depth 2)
                "0x0000000000000000000000000000000000000000000000000000000000003649": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "0x0000000000000000000000000000000000000000000000000000000000007651": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "0x000000000000000000000000000000000000000000000000000000000000b542": "0x0000000000000000000000000000000000000000000000000000000000000003",
                // Group 2: slots that hash to prefix 0x05f3 (creates branch at depth 2)
                "0x0000000000000000000000000000000000000000000000000000000000001cd9": "0x0000000000000000000000000000000000000000000000000000000000000004",
            }
        },
        // Test accounts with funds
        "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266": {
            "balance": "0xd3c21bcecceda1000000"
        },
        "0x70997970c51812dc3a010c7d01b50e0d17dc79c8": {
            "balance": "0xd3c21bcecceda1000000"
        }
    });

    serde_json::json!({
        "config": {
            "chainId": 1,
            "homesteadBlock": 0,
            "daoForkSupport": true,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0,
            "muirGlacierBlock": 0,
            "berlinBlock": 0,
            "londonBlock": 0,
            "arrowGlacierBlock": 0,
            "grayGlacierBlock": 0,
            "shanghaiTime": 0,
            "cancunTime": 0,
            "terminalTotalDifficulty": "0x0",
            "terminalTotalDifficultyPassed": true
        },
        "nonce": "0x0",
        "timestamp": "0x0",
        "extraData": "0x00",
        "gasLimit": "0x1c9c380",
        "difficulty": "0x0",
        "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "coinbase": "0x0000000000000000000000000000000000000000",
        "alloc": alloc,
        "number": "0x0"
    })
}

/// Creates a transaction that modifies a storage slot in the test contract
async fn create_storage_tx_with_signer(
    slot: U256,
    value: U256,
    nonce: u64,
    private_key: &str,
) -> Result<Bytes> {
    let signer = PrivateKeySigner::from_str(private_key)?;

    let mut calldata = vec![0u8; 64];
    let slot_bytes = slot.to_be_bytes::<32>();
    let value_bytes = value.to_be_bytes::<32>();
    calldata[0..32].copy_from_slice(&slot_bytes);
    calldata[32..64].copy_from_slice(&value_bytes);

    let tx_request = TransactionRequest {
        nonce: Some(nonce),
        value: Some(U256::ZERO),
        to: Some(TxKind::Call(STORAGE_CONTRACT)),
        gas: Some(100_000),
        max_fee_per_gas: Some(20_000_000_000),
        max_priority_fee_per_gas: Some(20_000_000_000),
        chain_id: Some(1),
        input: TransactionInput { input: None, data: Some(Bytes::from(calldata)) },
        ..Default::default()
    };

    let wallet = EthereumWallet::from(signer);
    let signed_tx =
        <TransactionRequest as TransactionBuilder<Ethereum>>::build(tx_request, &wallet).await?;

    Ok(signed_tx.encoded_2718().into())
}

/// Creates the test environment setup with proper configuration for trie testing.
///
/// This setup includes:
/// - A chain spec with pre-deployed storage contract and test accounts
/// - Single node network configuration
/// - Tree config with legacy state root to ensure trie updates are preserved
///
/// The configuration uses `legacy_state_root` to bypass optimizations that
/// would otherwise discard trie updates for non-persisted blocks.
fn create_test_setup() -> Setup<EthEngineTypes> {
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_value(create_test_genesis()).unwrap())
            .cancun_activated()
            .build(),
    );

    Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node())
        .with_tree_config(
            TreeConfig::default()
                .with_persistence_threshold(0) // Persist blocks immediately so trie updates are kept
                .with_legacy_state_root(true), /* Use legacy state root to bypass trie update
                                                * optimizations */
        )
}

/// Initializes storage slots that deliberately create branch nodes in the trie.
///
/// The slot values are carefully chosen so their keccak256 hashes share common prefixes:
/// - Slots a, b, c hash to prefix 0x70e0, creating a branch node at depth 2
/// - Slot d hashes to prefix 0x05f3, creating another branch node
///
/// This trie structure helps test that trie updates are properly preserved.
fn init_storage_slots() -> StorageSlots {
    StorageSlots {
        // Group 1: Slots hashing to prefix 0x70e0 (creates branch at depth 2)
        slot_a: U256::from(0x3649),
        slot_b: U256::from(0x7651),
        slot_c: U256::from(0xb542),
        // Group 2: Slots hashing to prefix 0x05f3 (creates another branch)
        slot_d: U256::from(0x1cd9),
    }
}

/// Initializes storage values for testing trie updates.
///
/// The values are designed to:
/// - Set initial state in block 1
/// - Modify that state in block 2 to verify trie updates are preserved
fn init_storage_values() -> StorageValues {
    StorageValues {
        // Block 1: Set initial values
        block_1: BlockStorageValues {
            slot_a: U256::from(0x1111),
            slot_b: U256::from(0x2222),
            slot_c: U256::from(0x3333),
            slot_d: U256::from(0x4444),
        },
        // Block 2: Modify all values to ensure trie updates
        block_2: BlockStorageValues {
            slot_a: U256::from(0x5555),
            slot_b: U256::from(0x6666),
            slot_c: U256::from(0x7777),
            slot_d: U256::from(0x8888),
        },
    }
}

/// Creates test transactions for the canonical chain.
///
/// Generates two sets of transactions:
/// 1. Block 1: Set initial storage values
/// 2. Block 2: Modify those values to test trie update preservation
///
/// Each transaction is signed with test private keys and properly sequenced with
/// incrementing nonces to ensure correct execution order.
async fn create_test_transactions(
    slots: &StorageSlots,
    values: &StorageValues,
) -> Result<TestTransactions> {
    // Block 1 transactions: Set initial values
    let set_initial_values = vec![
        create_storage_tx_with_signer(slots.slot_a, values.block_1.slot_a, 0, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_b, values.block_1.slot_b, 1, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_c, values.block_1.slot_c, 2, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_d, values.block_1.slot_d, 3, TEST_PRIVATE_KEY_1)
            .await?,
    ];

    // Block 2 transactions: Modify values
    let modify_values = vec![
        create_storage_tx_with_signer(slots.slot_a, values.block_2.slot_a, 4, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_b, values.block_2.slot_b, 5, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_c, values.block_2.slot_c, 6, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_d, values.block_2.slot_d, 7, TEST_PRIVATE_KEY_1)
            .await?,
    ];

    Ok(TestTransactions { set_initial_values, modify_values })
}
