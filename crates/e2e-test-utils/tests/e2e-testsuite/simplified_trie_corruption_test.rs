//! Simplified test suite for reproducing the trie corruption bug
//!
//! This test creates a minimal scenario that reproduces the trie corruption bug
//! that occurred during reorgs, without needing real mainnet data.
//!
//! Bug scenario:
//! 1. Block 1 (canonical): Sets storage slot 0x0f to value A
//! 2. Block 1' (fork): Sets storage slot 0x0f to value B (creates trie node)
//! 3. Reorg: Make Block 1' canonical
//! 4. Block 2: Clears storage slot 0x0f (should delete trie node created in Block 1')
//!
//! Expected bug: Block 2's trie updates will be missing the deletion of node 0x0f
//! because Block 1' (the fork block) had its trie updates discarded.

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{CaptureBlock, ProduceBlockWithTransactionsViaEngineAPI, ReorgTo},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_api::TreeConfig;
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use std::{str::FromStr, sync::Arc};
use tracing::info;

mod verify_any_trie_updates;
use verify_any_trie_updates::VerifyAnyTrieUpdatesEmitted;

mod assert_branch_node_update;
use assert_branch_node_update::AssertBranchNodeAtPrefix;

mod assert_missing_trie_updates;
use assert_missing_trie_updates::AssertMissingTrieUpdates;

mod send_valid_fork_block;
use send_valid_fork_block::SendValidForkBlock;

/// Storage contract address for our tests
const STORAGE_CONTRACT: &str = "0x1234567890123456789012345678901234567890";

/// Test account private keys (from common test accounts)
const TEST_PRIVATE_KEY_1: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const TEST_PRIVATE_KEY_2: &str =
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

#[tokio::test]
async fn test_simplified_trie_corruption_bug_reproduction() -> Result<()> {
    reth_tracing::init_test_tracing();

    println!("STARTING SIMPLIFIED TRIE CORRUPTION TEST");
    println!("Genesis will have:");
    println!("  - 40+ accounts with similar prefixes");
    println!("  - 28 pre-populated storage slots in the contract");
    println!("This should create a complex trie structure with branch nodes");

    // Create chain spec with storage contract pre-deployed for our test
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_value(create_test_genesis()).unwrap())
            .cancun_activated()
            .build(),
    );

    let setup = Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node())
        .with_tree_config(
            TreeConfig::default()
                .with_state_root_fallback(false)
                .with_always_compare_trie_updates(true),
        );

    // Use optimized storage slots that hash to common prefixes, creating deeper trie branches
    // These slots all hash to prefix 0x70e0, creating a branch node at depth 2
    let slot_group1_a = U256::from(0x3649); // Slot A (will get same value in both blocks)
    let slot_group1_b = U256::from(0x7651); // Slot B (will get different values)
    let slot_group1_c = U256::from(0xb542); // Slot C (not used in fork, for variety)
                                            // These slots hash to prefix 0x05f3, creating another branch at depth 2
    let slot_group2_a = U256::from(0x1cd9); // Slot D (will get different values)

    // Values for canonical block 1
    let value_1111 = U256::from(0x1111); // For slot A in both blocks (overlap)
    let value_2222 = U256::from(0x2222); // For slot B in canonical
    let value_3333 = U256::from(0x3333); // For slot C in canonical
    let value_4444 = U256::from(0x4444); // For slot D in canonical

    // Values for fork block 1'
    // Slot A: same value (0x1111) - partial state overlap
    let value_5555 = U256::from(0x5555); // For slot B in fork (DIFFERENT!)
    let value_6666 = U256::from(0x6666); // For slot D in fork (DIFFERENT!)

    let value_zero = U256::ZERO; // For deletions in block 2

    // === CANONICAL BLOCK 1 TRANSACTIONS ===
    // These set initial values in the storage
    let tx1_canonical_a =
        create_storage_tx_with_signer(slot_group1_a, value_1111, 0, TEST_PRIVATE_KEY_1).await?;
    let tx1_canonical_b =
        create_storage_tx_with_signer(slot_group1_b, value_2222, 1, TEST_PRIVATE_KEY_1).await?;
    let tx1_canonical_c =
        create_storage_tx_with_signer(slot_group1_c, value_3333, 2, TEST_PRIVATE_KEY_1).await?;
    let tx1_canonical_d =
        create_storage_tx_with_signer(slot_group2_a, value_4444, 3, TEST_PRIVATE_KEY_1).await?;

    // === FORK BLOCK 1' TRANSACTIONS ===
    // Same slots but DIFFERENT values (except slot A for partial overlap)
    let tx1_fork_a =
        create_storage_tx_with_signer(slot_group1_a, value_1111, 0, TEST_PRIVATE_KEY_2).await?; // SAME value as canonical
    let tx1_fork_b =
        create_storage_tx_with_signer(slot_group1_b, value_5555, 1, TEST_PRIVATE_KEY_2).await?; // DIFFERENT value!
    let tx1_fork_d =
        create_storage_tx_with_signer(slot_group2_a, value_6666, 2, TEST_PRIVATE_KEY_2).await?; // DIFFERENT value!

    // Next block: delete values from deep branch nodes
    let tx2_delete =
        create_storage_tx_with_signer(slot_group1_a, value_zero, 2, TEST_PRIVATE_KEY_2).await?;
    let tx2b_delete =
        create_storage_tx_with_signer(slot_group2_a, value_zero, 3, TEST_PRIVATE_KEY_2).await?;

    let storage_contract_addr = Address::from_str(STORAGE_CONTRACT)?;

    let test = TestBuilder::new()
        .with_setup(setup)
        // Step 1: Send canonical block 1 via Engine API with initial values
        .with_action({
            info!("Step 1: Sending canonical block 1 via Engine API...");
            info!("  Slots: A=0x1111, B=0x2222, C=0x3333, D=0x4444");
            ProduceBlockWithTransactionsViaEngineAPI::new(
                vec![
                    tx1_canonical_a.clone(),
                    tx1_canonical_b.clone(),
                    tx1_canonical_c.clone(),
                    tx1_canonical_d.clone(),
                ],
                "block_1_canonical",
            )
        })
        .with_action(CaptureBlock::new("block_1_canonical"))
        .with_action(VerifyAnyTrieUpdatesEmitted::new())
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
        // Step 2: Send fork block 1' as a valid competing block (same height, different state)
        // This creates a fork at the same height as block_1_canonical
        .with_action({
            info!("Step 2: Sending valid fork block 1'...");
            info!("  Slots: A=0x1111 (SAME), B=0x5555 (DIFFERENT!), D=0x6666 (DIFFERENT!)");
            info!("  This creates a valid fork that competes with block_1_canonical");
            SendValidForkBlock::new(
                vec![
                    tx1_fork_a.clone(), // Slot A: same value as canonical
                    tx1_fork_b.clone(), // Slot B: DIFFERENT value!
                    tx1_fork_d.clone(), // Slot D: DIFFERENT value!
                ],
                "block_1_fork",
            )
        })
        // Step 3: Verify that the fork block has MISSING trie updates
        // This is the key bug condition - fork blocks get ExecutedTrieUpdates::Missing
        .with_action({
            info!("Step 3: Checking if fork block has missing trie updates (bug condition)...");
            AssertMissingTrieUpdates::new("block_1_fork").expect_missing(true) // With bug, fork
                                                                               // blocks should have
                                                                               // missing updates
        })
        // Step 4: Now make the fork canonical via reorg
        .with_action({
            info!("Step 4: Triggering reorg to make fork block canonical...");
            ReorgTo::new_from_tag("block_1_fork")
        })
        // Step 5: Send block 2 that builds on the fork
        .with_action({
            info!("Step 5: Sending block 2 with deletion transactions...");
            ProduceBlockWithTransactionsViaEngineAPI::new(
                vec![tx2_delete.clone(), tx2b_delete.clone()],
                "block_2_after_reorg",
            )
        })
        // Step 6: Check the bug manifestation
        // With the bug, deletions won't be properly tracked because fork block's trie updates were
        // missing
        .with_action({
            info!("Step 6: Checking if deletions are missing (bug manifestation)...");
            // The bug causes missing deletions - the 0x70e0 branch won't be in removed_nodes
            AssertBranchNodeAtPrefix::new("block_2_after_reorg", storage_contract_addr, "70e0")
                .expect_present(false) // Node should be gone
                                       // BUT the removal might not be tracked properly due to the
                                       // bug
        });

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Create a complex genesis with storage contract pre-deployed and multiple accounts
/// This creates a more complex trie structure with branch nodes
fn create_test_genesis() -> serde_json::Value {
    // Create multiple accounts with similar prefixes to force branch node creation
    let mut alloc = serde_json::json!({
        STORAGE_CONTRACT: {
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

/// Create a transaction that sets storage slot 0xf to a specific value
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
        to: Some(TxKind::Call(Address::from_str(STORAGE_CONTRACT)?)),
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
