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
use alloy_primitives::{Address, Bytes, TxKind, U256};
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

// Trie-related actions are now available from the main actions module

/// Storage contract address for our tests
const STORAGE_CONTRACT: &str = "0x1234567890123456789012345678901234567890";

/// Test account private keys (from common test accounts)
const TEST_PRIVATE_KEY_1: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const TEST_PRIVATE_KEY_2: &str =
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

#[tokio::test]
async fn test_trie_updates_during_fork_and_reorg() -> Result<()> {
    reth_tracing::init_test_tracing();

    println!("\n=== TRIE FORK AND REORG TEST ===");
    println!("Testing trie updates when:");
    println!("  1. Creating fork blocks with different storage values");
    println!("  2. Performing reorgs between chains");
    println!("  3. Deleting storage after reorg");
    println!("\nGenesis setup creates complex trie with branch nodes.");

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

    // ========================================
    // STORAGE SLOT CONFIGURATION
    // ========================================
    // Slots are chosen to hash to common prefixes, creating branch nodes in the trie
    
    // Group 1: Slots hashing to prefix 0x70e0 (creates branch at depth 2)
    let slot_a = U256::from(0x3649); // Same value in both canonical and fork
    let slot_b = U256::from(0x7651); // Different values in canonical vs fork
    let slot_c = U256::from(0xb542); // Only used in canonical chain
    
    // Group 2: Slots hashing to prefix 0x05f3 (creates another branch)
    let slot_d = U256::from(0x1cd9); // Different values in canonical vs fork

    // ========================================
    // STORAGE VALUES
    // ========================================
    
    // Canonical chain values (Block 1)
    let shared_value = U256::from(0x1111);     // Slot A: Same in both chains
    let canonical_b = U256::from(0x2222);      // Slot B: Canonical only
    let canonical_c = U256::from(0x3333);      // Slot C: Canonical only  
    let canonical_d = U256::from(0x4444);      // Slot D: Canonical only
    
    // Fork chain values (Block 1')
    let fork_b = U256::from(0x5555);           // Slot B: Different in fork
    let fork_d = U256::from(0x6666);           // Slot D: Different in fork
    
    // Deletion value
    let zero = U256::ZERO;

    // ========================================
    // TRANSACTION CREATION
    // ========================================
    
    // Canonical Block 1 transactions
    let canonical_txs = vec![
        create_storage_tx_with_signer(slot_a, shared_value, 0, TEST_PRIVATE_KEY_1).await?,
        create_storage_tx_with_signer(slot_b, canonical_b, 1, TEST_PRIVATE_KEY_1).await?,
        create_storage_tx_with_signer(slot_c, canonical_c, 2, TEST_PRIVATE_KEY_1).await?,
        create_storage_tx_with_signer(slot_d, canonical_d, 3, TEST_PRIVATE_KEY_1).await?,
    ];

    // Fork Block 1' transactions (different values for slots B and D)
    let fork_txs = vec![
        create_storage_tx_with_signer(slot_a, shared_value, 0, TEST_PRIVATE_KEY_2).await?, // Same as canonical
        create_storage_tx_with_signer(slot_b, fork_b, 1, TEST_PRIVATE_KEY_2).await?,       // Different!
        create_storage_tx_with_signer(slot_d, fork_d, 2, TEST_PRIVATE_KEY_2).await?,       // Different!
    ];

    // Block 2 transactions: Delete storage slots
    let deletion_txs = vec![
        create_storage_tx_with_signer(slot_a, zero, 2, TEST_PRIVATE_KEY_2).await?,
        create_storage_tx_with_signer(slot_d, zero, 3, TEST_PRIVATE_KEY_2).await?,
    ];

    let storage_contract_addr = Address::from_str(STORAGE_CONTRACT)?;

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
                canonical_txs.clone(),
                "block_1_canonical",
            )
        })
        .with_action(CaptureBlock::new("block_1_canonical"))
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
            SendValidForkBlock::new(
                fork_txs.clone(),
                "block_1_fork",
            )
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
        
        // ----------------------------------------
        // PHASE 4: Delete storage after reorg
        // ----------------------------------------
        .with_action({
            info!("Creating Block 2 with storage deletions...");
            ProduceBlockWithTransactionsViaEngineAPI::new(
                deletion_txs.clone(),
                "block_2_after_reorg",
            )
        })
        
        // ----------------------------------------
        // PHASE 5: Verify bug manifestation
        // ----------------------------------------
        .with_action({
            info!("Verifying trie node deletions after reorg...");
            info!("Bug manifests as missing deletion tracking for branch nodes.");
            AssertBranchNodeAtPrefix::new("block_2_after_reorg", storage_contract_addr, "70e0")
                .expect_present(false) // Should be deleted, but bug may prevent proper tracking
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
