//! E2E tests for SELFDESTRUCT behavior and output state verification.
//!
//! These tests verify that:
//! - Pre-Dencun: SELFDESTRUCT clears storage and code, output state reflects this
//! - Post-Dencun (EIP-6780): SELFDESTRUCT only works in same-tx creation, state persists
//!
//! We disable prewarming to ensure deterministic cache behavior and verify the execution
//! output state contains the expected account status after SELFDESTRUCT.

use crate::utils::{eth_payload_attributes, eth_payload_attributes_shanghai};
use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{bytes, Bytes, TxKind, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use futures::StreamExt;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_api::TreeConfig;
use reth_node_ethereum::EthereumNode;
use reth_revm::db::BundleAccount;
use std::sync::Arc;

/// Creates init code for a simple contract that:
/// 1. Stores 0x42 at slot 0 during deployment
/// 2. On any call: selfdestructs to beneficiary
///
/// This simpler contract avoids complex branching logic.
fn selfdestruct_contract_init_code() -> Bytes {
    // Runtime: just selfdestruct on any call
    // PUSH20 <beneficiary>
    // SELFDESTRUCT
    let runtime = bytes!(
        "73dead000000000000000000000000000000000001" // PUSH20 beneficiary
        "ff"       // SELFDESTRUCT
    );

    let runtime_len = runtime.len(); // 22 bytes

    // Init code: SSTORE(0, 0x42), CODECOPY, RETURN
    // Total init code before runtime = 17 bytes
    let init_len: u8 = 17;

    let mut init = Vec::new();
    init.extend_from_slice(&[0x60, 0x42, 0x60, 0x00, 0x55]); // PUSH1 0x42, PUSH1 0x00, SSTORE
    init.extend_from_slice(&[0x60, runtime_len as u8, 0x60, init_len, 0x60, 0x00, 0x39]); // CODECOPY
    init.extend_from_slice(&[0x60, runtime_len as u8, 0x60, 0x00, 0xf3]); // RETURN
    init.extend_from_slice(&runtime);

    Bytes::from(init)
}

/// Tests SELFDESTRUCT behavior post-Dencun (Cancun+).
///
/// Post-Dencun (EIP-6780):
/// - SELFDESTRUCT only deletes contract if called in same tx as creation
/// - For existing contracts, SELFDESTRUCT only sends balance, code/storage persist
/// - The output state should NOT mark the account as destroyed
///
/// This test verifies:
/// 1. Contract deploys with storage
/// 2. SELFDESTRUCT in later tx does NOT delete code/storage
/// 3. Output state shows account is NOT destroyed
#[tokio::test]
async fn test_selfdestruct_post_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Cancun activated (post-Dencun, EIP-6780)
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let tree_config = TreeConfig::default()
        // Disable prewarming
        .without_prewarming(true)
        // Enable state cache
        .without_state_cache(false);

    let (mut nodes, _tasks, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        tree_config,
        eth_payload_attributes,
    )
    .await?;

    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Deploy contract that stores 0x42 at slot 0 and selfdestructs on any call
    let init_code = selfdestruct_contract_init_code();
    let deploy_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_nonce(0)
        .with_gas_limit(500_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128)
        .with_input(init_code)
        .with_kind(TxKind::Create);

    let pending = provider.send_transaction(deploy_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment should succeed");

    let contract_address = receipt.contract_address.expect("Should have contract address");

    // Consume the canonical notification for deployment block
    let _ = node.canonical_stream.next().await;

    // Trigger SELFDESTRUCT by calling the contract
    let selfdestruct_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_to(contract_address)
        .with_nonce(1)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128);

    let pending = provider.send_transaction(selfdestruct_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Selfdestruct tx should succeed");

    // Get the canonical notification for the selfdestruct block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: post-Dencun, account should NOT be destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_none() || !account_state.unwrap().was_destroyed(),
        "Post-Dencun (EIP-6780): Account should NOT be destroyed when SELFDESTRUCT called on existing contract"
    );

    // Verify via RPC that code and storage persist
    let code_after = provider.get_code_at(contract_address).await?;
    assert!(!code_after.is_empty(), "Post-Dencun: Contract code should persist");

    let slot0_after = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_after, U256::from(0x42), "Post-Dencun: Storage should persist");

    // Send another transaction to the contract address in a new block.
    // This tests cache behavior - if cache has stale data, execution would be incorrect.
    // Post-Dencun: calling the contract should trigger SELFDESTRUCT again (but only transfer balance)
    let call_again_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_to(contract_address)
        .with_nonce(2)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128);

    let pending = provider.send_transaction(call_again_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Second call to contract should succeed");

    // Consume the canonical notification
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state still shows account NOT destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_none() || !account_state.unwrap().was_destroyed(),
        "Post-Dencun: Account should still NOT be destroyed after second SELFDESTRUCT call"
    );

    // Verify code and storage still persist after the second call
    let code_final = provider.get_code_at(contract_address).await?;
    assert!(!code_final.is_empty(), "Post-Dencun: Contract code should still persist");

    let slot0_final = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_final, U256::from(0x42), "Post-Dencun: Storage should still persist");

    Ok(())
}

/// Tests SELFDESTRUCT behavior pre-Dencun (Shanghai).
///
/// Pre-Dencun:
/// - SELFDESTRUCT deletes contract code and storage regardless of when contract was created
/// - The output state MUST mark the account as destroyed
///
/// This test verifies:
/// 1. Contract deploys with storage
/// 2. SELFDESTRUCT deletes code and storage
/// 3. Output state shows account IS destroyed
#[tokio::test]
async fn test_selfdestruct_pre_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Shanghai activated (pre-Dencun, original SELFDESTRUCT behavior)
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .shanghai_activated()
            .build(),
    );

    let tree_config = TreeConfig::default()
        // Disable prewarming
        .without_prewarming(true)
        // Enable state cache
        .without_state_cache(false);

    let (mut nodes, _tasks, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        tree_config,
        eth_payload_attributes_shanghai,
    )
    .await?;

    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Deploy contract that stores 0x42 at slot 0 and selfdestructs on any call
    let init_code = selfdestruct_contract_init_code();
    let deploy_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_nonce(0)
        .with_gas_limit(500_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128)
        .with_input(init_code)
        .with_kind(TxKind::Create);

    let pending = provider.send_transaction(deploy_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment should succeed");

    let contract_address = receipt.contract_address.expect("Should have contract address");

    // Consume the canonical notification for deployment block
    let _ = node.canonical_stream.next().await;

    // Trigger SELFDESTRUCT by calling the contract
    let selfdestruct_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_to(contract_address)
        .with_nonce(1)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128);

    let pending = provider.send_transaction(selfdestruct_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Selfdestruct tx should succeed");

    // Get the canonical notification for the selfdestruct block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: pre-Dencun, account MUST be destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_some_and(|a: &BundleAccount| a.was_destroyed()),
        "Pre-Dencun: Account MUST be marked as destroyed in output state"
    );

    // Verify via RPC that code and storage are cleared
    let code_after = provider.get_code_at(contract_address).await?;
    assert!(code_after.is_empty(), "Pre-Dencun: Contract code should be deleted");

    let slot0_after = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_after, U256::ZERO, "Pre-Dencun: Storage should be cleared");

    // Send ETH to the destroyed contract address in a new block.
    // This tests cache behavior - the cache should correctly reflect the account was destroyed.
    // Pre-Dencun: the contract no longer exists, so this is just a plain ETH transfer.
    let send_eth_tx = TransactionRequest::default()
        .with_from(signer.address())
        .with_to(contract_address)
        .with_nonce(2)
        .with_value(U256::from(1000))
        .with_gas_limit(21_000)
        .with_max_fee_per_gas(20_000_000_000_u128)
        .with_max_priority_fee_per_gas(1_000_000_000_u128);

    let pending = provider.send_transaction(send_eth_tx).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "ETH transfer to destroyed contract address should succeed");

    // Consume the canonical notification
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state shows the account exists (received ETH) but has no code
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    // After receiving ETH, the account should exist with balance but no code
    assert!(
        account_state.is_some(),
        "Pre-Dencun: Account should exist after receiving ETH (even though contract was destroyed)"
    );

    // Verify code is still empty (contract was destroyed, only ETH was received)
    let code_final = provider.get_code_at(contract_address).await?;
    assert!(code_final.is_empty(), "Pre-Dencun: Contract code should remain deleted");

    // Verify storage is still cleared
    let slot0_final = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_final, U256::ZERO, "Pre-Dencun: Storage should remain cleared");

    // Verify the account now has the ETH balance we sent
    let balance = provider.get_balance(contract_address).await?;
    assert_eq!(balance, U256::from(1000), "Pre-Dencun: Account should have received ETH");

    Ok(())
}
