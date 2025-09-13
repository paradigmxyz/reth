//! Test suite for validating trie updates during fork and reorg scenarios.
//!
//! This test verifies that trie nodes are properly updated when:
//! - Fork blocks are created with different storage values
//! - Reorgs switch between canonical and fork chains
//! - Storage slots are deleted after a reorg
//!
//! The test specifically checks for a historical bug where fork blocks
//! would have missing trie updates, causing incorrect state after reorgs.

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, TransactionBuilder};
use alloy_primitives::{address, Address, Bytes, TxKind, U256};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        AssertBranchNodeAtPrefix, AssertMissingTrieUpdates, CaptureBlock,
        ProduceBlockWithTransactionsViaEngineAPI, ReorgTo, SendValidForkBlock,
        VerifyAnyTrieUpdatesEmitted,
    },
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_api::TreeConfig;
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use std::{str::FromStr, sync::Arc};
use tracing::info;

/// Storage contract address for our tests
const STORAGE_CONTRACT: Address = address!("1234567890123456789012345678901234567890");

/// Test account private keys (from common test accounts)
const TEST_PRIVATE_KEY_1: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const TEST_PRIVATE_KEY_2: &str =
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

/// Storage slot configuration for trie fork tests.
///
/// These slots are specifically chosen to create branch nodes in the Merkle Patricia Trie
/// by having common hash prefixes. This allows us to test trie update behavior during
/// fork and reorg scenarios.
struct StorageSlots {
    slot_a: U256, // Group 1: 0x70e0 prefix - Same value in both chains
    slot_b: U256, // Group 1: 0x70e0 prefix - Different values
    slot_c: U256, // Group 1: 0x70e0 prefix - Canonical only
    slot_d: U256, // Group 2: 0x05f3 prefix - Different values
}

/// Storage values used to test different chain states.
///
/// These values are designed to create specific trie node patterns:
/// - Shared values test partial state overlap between chains
/// - Different values force trie node updates during reorgs
/// - Zero values (via deletion) test trie node removal
struct StorageValues {
    shared: U256,      // Used in both chains
    canonical_b: U256, // Canonical chain value for slot B
    canonical_c: U256, // Canonical chain value for slot C
    canonical_d: U256, // Canonical chain value for slot D
    fork_b: U256,      // Fork chain value for slot B
    fork_d: U256,      // Fork chain value for slot D
}

/// Test transactions for different blocks
struct TestTransactions {
    /// Transactions for canonical Block 1 (sets initial storage values)
    canonical: Vec<Bytes>,
    /// Transactions for fork Block 1' (creates competing state)
    fork: Vec<Bytes>,
    /// Transactions for Block 2 (deletes storage after reorg)
    deletion: Vec<Bytes>,
}

/// Test trie updates during fork and reorg scenarios
#[tokio::test]
async fn test_trie_updates_during_fork_and_reorg() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Initialize test configuration
    let setup = create_test_setup();
    let slots = init_storage_slots();
    let values = init_storage_values();
    let txs = create_test_transactions(&slots, &values).await?;
    let storage_contract_addr = STORAGE_CONTRACT;

    // ========================================
    // TEST EXECUTION
    // ========================================

    let test = TestBuilder::new()
        .with_setup(setup)
        // ----------------------------------------
        // PHASE 1: Create canonical chain
        // ----------------------------------------
        .with_action({
            info!("Creating canonical Block 1 with storage values:");
            info!("  Slot A: 0x1111, B: 0x2222, C: 0x3333, D: 0x4444");
            ProduceBlockWithTransactionsViaEngineAPI::new(
                txs.canonical.clone(),
                "block_1_canonical",
            )
        })
        .with_action(CaptureBlock::new("block_1_canonical"))
        // Verify canonical block has trie updates present
        .with_action({
            info!("Verifying canonical block 1 has trie updates...");
            AssertMissingTrieUpdates::new("block_1_canonical").expect_missing(false)
        })
        .with_action(VerifyAnyTrieUpdatesEmitted::new())
        // Verify branch nodes were created
        .with_action(
            AssertBranchNodeAtPrefix::new("block_1_canonical", storage_contract_addr, "70e0")
                .expect_present(true)
                .with_debug_contains("BranchNodeCompact"),
        )
        .with_action(
            AssertBranchNodeAtPrefix::new("block_1_canonical", storage_contract_addr, "05f3")
                .expect_present(true)
                .with_debug_contains("BranchNodeCompact"),
        )
        // ----------------------------------------
        // PHASE 2: Create competing fork
        // ----------------------------------------
        .with_action({
            info!("Creating fork Block 1' with different storage:");
            info!("  Slot A: 0x1111 (same), B: 0x5555 (different), D: 0x6666 (different)");
            SendValidForkBlock::new(txs.fork.clone(), "block_1_fork")
        })
        // Verify fork block has missing trie updates (the bug condition)
        .with_action({
            info!("Checking fork block trie updates (expecting missing due to bug)...");
            AssertMissingTrieUpdates::new("block_1_fork").expect_missing(true)
        })
        // ----------------------------------------
        // PHASE 3: Reorg to fork chain
        // ----------------------------------------
        .with_action({
            info!("Triggering reorg to make fork chain canonical...");
            ReorgTo::new_from_tag("block_1_fork")
        })
        // After reorg, the fork block should now be canonical but still have missing trie updates
        // This is a key part of the bug - the trie updates don't get regenerated after reorg
        .with_action({
            info!(
                "Verifying fork block still has missing trie updates after becoming canonical..."
            );
            AssertMissingTrieUpdates::new("block_1_fork").expect_missing(true)
        })
        // ----------------------------------------
        // PHASE 4: Delete storage after reorg
        // ----------------------------------------
        .with_action({
            info!("Creating Block 2 with storage deletions...");
            ProduceBlockWithTransactionsViaEngineAPI::new(
                txs.deletion.clone(),
                "block_2_after_reorg",
            )
        })
        .with_action(CaptureBlock::new("block_2_after_reorg"))
        // Verify block 2 has trie updates (it should, since it's built on canonical chain)
        .with_action({
            info!("Verifying Block 2 has trie updates present...");
            AssertMissingTrieUpdates::new("block_2_after_reorg").expect_missing(false)
        })
        // ----------------------------------------
        // PHASE 5: Verify bug manifestation
        // ----------------------------------------
        .with_action({
            info!("Verifying trie node deletions after reorg...");
            info!("Bug manifests as missing deletion tracking for branch nodes.");
            info!("The branch node at 0x70e0 should be deleted, but may not be properly tracked");
            info!("due to the missing trie updates from the fork block (block_1_fork).");
            AssertBranchNodeAtPrefix::new("block_2_after_reorg", storage_contract_addr, "70e0")
                .expect_present(false) // Should be deleted, but bug may prevent proper tracking
        })
        // Also check the other branch node that should still exist
        .with_action({
            info!("Verifying branch node at 0x05f3 is properly removed...");
            AssertBranchNodeAtPrefix::new("block_2_after_reorg", storage_contract_addr, "05f3")
                .expect_present(false) // This one should also be deleted since slot D was cleared
        });

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Creates a genesis state with:
/// - A storage contract with pre-populated slots creating branch nodes
/// - Multiple accounts with common address prefixes to force trie branching
fn create_test_genesis() -> serde_json::Value {
    // Create multiple accounts with similar prefixes to force branch node creation
    let mut alloc = serde_json::json!({
        format!("{:?}", STORAGE_CONTRACT): {
            "balance": "0x0",
            "code": "0x6000356020359055",
            // Optimized storage slots that create deeper trie structures by having common hash prefixes
            "storage": {
                // Group 1: slots that hash to prefix 0x70e0 (creates branch at depth 2)
                "0x00000000000000000000000000000000000000000000000000000000000047b2": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "0x0000000000000000000000000000000000000000000000000000000000007651": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "0x000000000000000000000000000000000000000000000000000000000000b542": "0x0000000000000000000000000000000000000000000000000000000000000003",
                "0x000000000000000000000000000000000000000000000000000000000000bb5c": "0x0000000000000000000000000000000000000000000000000000000000000004",
                "0x000000000000000000000000000000000000000000000000000000000000c86f": "0x0000000000000000000000000000000000000000000000000000000000000005",
                // Group 2: slots that hash to prefix 0x05f3 (creates branch at depth 2)
                "0x0000000000000000000000000000000000000000000000000000000000001cd9": "0x0000000000000000000000000000000000000000000000000000000000000006",
                "0x00000000000000000000000000000000000000000000000000000000000040aa": "0x0000000000000000000000000000000000000000000000000000000000000007",
                "0x00000000000000000000000000000000000000000000000000000000000083b3": "0x0000000000000000000000000000000000000000000000000000000000000008",
                "0x000000000000000000000000000000000000000000000000000000000000d519": "0x0000000000000000000000000000000000000000000000000000000000000009",
                "0x000000000000000000000000000000000000000000000000000000000000daa2": "0x000000000000000000000000000000000000000000000000000000000000000a",
                // Group 3: slots that hash to prefix 0xbd7b (creates branch at depth 2)
                "0x000000000000000000000000000000000000000000000000000000000000388b": "0x000000000000000000000000000000000000000000000000000000000000000b",
                "0x00000000000000000000000000000000000000000000000000000000000067a2": "0x000000000000000000000000000000000000000000000000000000000000000c",
                "0x00000000000000000000000000000000000000000000000000000000000093ab": "0x000000000000000000000000000000000000000000000000000000000000000d",
                "0x00000000000000000000000000000000000000000000000000000000000113fd": "0x000000000000000000000000000000000000000000000000000000000000000e",
                "0x00000000000000000000000000000000000000000000000000000000000120a4": "0x000000000000000000000000000000000000000000000000000000000000000f",
                // Group 4: slots that hash to prefix 0xb5cb (creates branch at depth 2)
                "0x00000000000000000000000000000000000000000000000000000000000011bb": "0x0000000000000000000000000000000000000000000000000000000000000010",
                "0x0000000000000000000000000000000000000000000000000000000000001ac4": "0x0000000000000000000000000000000000000000000000000000000000000011",
                "0x000000000000000000000000000000000000000000000000000000000000398d": "0x0000000000000000000000000000000000000000000000000000000000000012",
                "0x0000000000000000000000000000000000000000000000000000000000009b7a": "0x0000000000000000000000000000000000000000000000000000000000000013",
                "0x0000000000000000000000000000000000000000000000000000000000009d47": "0x0000000000000000000000000000000000000000000000000000000000000014",
                // Additional slots for variety
                "0x0000000000000000000000000000000000000000000000000000000000000000": "0x000000000000000000000000000000000000000000000000000000000000001a",
                "0x0000000000000000000000000000000000000000000000000000000000000001": "0x000000000000000000000000000000000000000000000000000000000000001b",
                "0x0000000000000000000000000000000000000000000000000000000000000002": "0x000000000000000000000000000000000000000000000000000000000000001c"
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

    // Add many accounts with similar prefixes to force branch node creation
    // These addresses share common prefixes which will create branch nodes
    for i in 0..20 {
        let addr = format!("0x12345678901234567890123456789012345678{:02x}", i);
        alloc[addr] = serde_json::json!({
            "balance": "0x1000000000000000000",
            "nonce": format!("0x{:x}", i)
        });
    }

    // Add another group with different prefix
    for i in 0..20 {
        let addr = format!("0xabcdef012345678901234567890123456789ab{:02x}", i);
        alloc[addr] = serde_json::json!({
            "balance": "0x2000000000000000000",
            "nonce": format!("0x{:x}", i + 20)
        });
    }

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
/// - Tree config with trie update comparison enabled to detect the bug
///
/// The configuration specifically enables `always_compare_trie_updates` which
/// is crucial for detecting missing trie updates in fork blocks.
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
                .with_state_root_fallback(false)
                .with_always_compare_trie_updates(true),
        )
}

/// Initializes storage slots that deliberately create branch nodes in the trie.
///
/// The slot values are carefully chosen so their keccak256 hashes share common prefixes:
/// - Slots a, b, c hash to prefix 0x70e0, creating a branch node at depth 2
/// - Slot d hashes to prefix 0x05f3, creating another branch node
///
/// This trie structure is essential for reproducing the bug where fork blocks
/// have missing trie updates for branch nodes.
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

/// Initializes storage values that create specific test conditions.
///
/// The values are designed to:
/// - Create partial state overlap (shared value 0x1111)
/// - Force trie updates during reorgs (different values for b and d)
/// - Test trie node deletion (values will be set to zero in deletion phase)
///
/// These specific values help expose the bug where trie updates are missing
/// in fork blocks, leading to incorrect state after reorgs.
fn init_storage_values() -> StorageValues {
    StorageValues {
        shared: U256::from(0x1111),
        canonical_b: U256::from(0x2222),
        canonical_c: U256::from(0x3333),
        canonical_d: U256::from(0x4444),
        fork_b: U256::from(0x5555),
        fork_d: U256::from(0x6666),
    }
}

/// Creates all test transactions for the different phases of the test.
///
/// Generates three sets of transactions:
/// 1. Canonical transactions: Set up initial storage state with all slots populated
/// 2. Fork transactions: Create competing state with some overlapping values
/// 3. Deletion transactions: Clear storage slots to test trie node removal
///
/// Each transaction is signed with test private keys and properly sequenced with
/// incrementing nonces to ensure correct execution order.
async fn create_test_transactions(
    slots: &StorageSlots,
    values: &StorageValues,
) -> Result<TestTransactions> {
    // Canonical Block 1 transactions
    let canonical = vec![
        create_storage_tx_with_signer(slots.slot_a, values.shared, 0, TEST_PRIVATE_KEY_1).await?,
        create_storage_tx_with_signer(slots.slot_b, values.canonical_b, 1, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_c, values.canonical_c, 2, TEST_PRIVATE_KEY_1)
            .await?,
        create_storage_tx_with_signer(slots.slot_d, values.canonical_d, 3, TEST_PRIVATE_KEY_1)
            .await?,
    ];

    // Fork Block 1' transactions (different values for slots B and D)
    let fork = vec![
        create_storage_tx_with_signer(slots.slot_a, values.shared, 0, TEST_PRIVATE_KEY_2).await?,
        create_storage_tx_with_signer(slots.slot_b, values.fork_b, 1, TEST_PRIVATE_KEY_2).await?,
        create_storage_tx_with_signer(slots.slot_d, values.fork_d, 2, TEST_PRIVATE_KEY_2).await?,
    ];

    // Block 2 transactions: Delete storage slots
    let deletion = vec![
        create_storage_tx_with_signer(slots.slot_a, U256::ZERO, 2, TEST_PRIVATE_KEY_2).await?,
        create_storage_tx_with_signer(slots.slot_d, U256::ZERO, 3, TEST_PRIVATE_KEY_2).await?,
    ];

    Ok(TestTransactions { canonical, fork, deletion })
}
