//! Test suite for validating trie updates preservation during reorgs.
//!
//! This test reproduces the incident where trie updates were incorrectly discarded
//! during a reorg, causing storage trie corruption.

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, TransactionBuilder};
use alloy_primitives::{address, Address, Bytes, TxKind, U256};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{
        AssertMissingTrieUpdates, CaptureBlock, CreateFork, MakeCanonical,
        ProduceBlockWithTransactionsViaEngineAPI, ProduceBlocks, ReorgTo,
    },
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_api::TreeConfig;
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use std::{str::FromStr, sync::Arc};

/// Storage contract address for our tests
const STORAGE_CONTRACT: Address = address!("77d34361f991fa724ff1db9b1d760063a16770db");

/// Test account private key (from common test accounts)
const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Storage slot that will create a branch node at path 0xf when hashed
/// This mimics the incident where slot
/// 0x4193f419339d8d52872c2a4d6ee5a6eb14c99332daf66ad89d1882bf092892be
/// hashes to 0xf388169196abf4cd20a13828879f10b0985d92fb58854196a68dad916132a0e4 (prefix 0xf)
const STORAGE_SLOT: U256 = U256::from_be_slice(&[
    0x41, 0x93, 0xf4, 0x19, 0x33, 0x9d, 0x8d, 0x52, 0x87, 0x2c, 0x2a, 0x4d, 0x6e, 0xe5, 0xa6, 0xeb,
    0x14, 0xc9, 0x93, 0x32, 0xda, 0xf6, 0x6a, 0xd8, 0x9d, 0x18, 0x82, 0xbf, 0x09, 0x28, 0x92, 0xbe,
]);

/// Test that reproduces the trie update loss bug during reorgs
///
/// This test recreates the exact scenario from the incident:
/// 1. Block N arrives (canonical chain continues)
/// 2. Block N+1 (fork) arrives - creates a different chain
/// 3. Block N+1 (canonical) arrives - creates storage trie node at path 0xf
/// 4. Block N+2 arrives - should delete the node at 0xf
///
/// The bug was that trie updates from block N+1 (canonical) were incorrectly discarded
/// because Reth checked if it was a fork instead of checking if it connected to the DB tip.
#[tokio::test]
async fn test_trie_updates_preserved_during_reorg() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Create test setup
    let setup = create_test_setup();

    // Create transactions for the test scenario
    let create_storage_tx = create_storage_transaction(STORAGE_SLOT, U256::from(1), 0).await?;
    let delete_storage_tx = create_storage_transaction(STORAGE_SLOT, U256::ZERO, 1).await?;

    let test = TestBuilder::new()
        .with_setup(setup)
        // Create base chain: block 1
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        .with_action(MakeCanonical::new())
        .with_action(CaptureBlock::new("base_block"))
        // Create fork block (block 2 fork) with a dummy transaction
        .with_action(CreateFork::<EthEngineTypes>::new_from_tag("base_block", 1))
        .with_action(CaptureBlock::new("fork_block"))
        // Go back to base and create canonical block 2 that creates storage node
        .with_action(ReorgTo::<EthEngineTypes>::new_from_tag("base_block"))
        .with_action(ProduceBlockWithTransactionsViaEngineAPI::new(
            vec![create_storage_tx],
            "canonical_block_2",
        ))
        .with_action(CaptureBlock::new("canonical_block_2"))
        // CRITICAL: This block's trie updates were incorrectly discarded in the bug
        // because it didn't directly connect to the DB tip (fork_block was the tip)
        .with_action(AssertMissingTrieUpdates::new("canonical_block_2").expect_missing(false))
        // Make canonical block 2 the head
        .with_action(MakeCanonical::new())
        // Create block 3 that deletes the storage node
        .with_action(ProduceBlockWithTransactionsViaEngineAPI::new(
            vec![delete_storage_tx],
            "block_3",
        ))
        .with_action(CaptureBlock::new("block_3"))
        // This block should have trie updates that remove the node at 0xf
        // In the bug, this failed because block 2's trie updates were missing
        .with_action(AssertMissingTrieUpdates::new("block_3").expect_missing(false));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Creates a transaction that modifies a storage slot in the test contract
async fn create_storage_transaction(slot: U256, value: U256, nonce: u64) -> Result<Bytes> {
    let signer = PrivateKeySigner::from_str(TEST_PRIVATE_KEY)?;

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

/// Creates the test environment setup
fn create_test_setup() -> Setup<EthEngineTypes> {
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(create_test_genesis())
            .cancun_activated()
            .build(),
    );

    Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node())
        .with_tree_config(
            TreeConfig::default()
                .with_persistence_threshold(0) // Persist blocks immediately
                .with_legacy_state_root(true), // Use legacy state root to ensure trie updates
        )
}

/// Creates a genesis state with the storage contract and funded test account
fn create_test_genesis() -> serde_json::Value {
    serde_json::json!({
        "config": {
            "chainId": 1,
            "homesteadBlock": 0,
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
        "alloc": {
            "0x77d34361f991fa724ff1db9b1d760063a16770db": {
                "balance": "0x0",
                "code": "0x6000356020359055"
            },
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266": {
                "balance": "0xd3c21bcecceda1000000"
            }
        },
        "number": "0x0"
    })
}
